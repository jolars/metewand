//! Transient terminal feedback kept separate from machine output and run evidence.

use crate::{
    core::{
        planning::LogicalPlan,
        run_output::{RunAttemptStatus, RunEvent, RunEventKind},
    },
    runtime::{
        repository::CheckedRepository,
        run::{PreparationPhase, PreparationProgress},
    },
};
use futures::{
    StreamExt,
    channel::mpsc::{self, UnboundedReceiver, UnboundedSender},
};
use iocraft::prelude::*;
use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(super) struct Progress {
    sender: Option<UnboundedSender<Message>>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

#[derive(Clone)]
struct Label {
    text: String,
    group: String,
}
impl From<&str> for Label {
    fn from(text: &str) -> Self {
        Self {
            text: text.into(),
            group: text.into(),
        }
    }
}
#[derive(Default)]
struct BenchmarkBar {
    total: usize,
    completed: usize,
    failed: usize,
}

enum Message {
    Outcome {
        slot: String,
        status: RunAttemptStatus,
        diagnostic: Option<crate::core::run_output::RunDiagnostic>,
    },
    Stage(String),
    Phase(PreparationProgress),
    Scheduled(Vec<String>),
    Labels(BTreeMap<String, Label>),
    Event(RunEvent),
    Tick,
    Stop,
}

impl Progress {
    pub fn new(enabled: bool, limit: Option<u64>) -> Self {
        if !enabled {
            return Self {
                sender: None,
                thread: None,
            };
        }
        let terminal = io::stderr().is_terminal() && std::env::var("TERM").as_deref() != Ok("dumb");
        let color = terminal && std::env::var_os("NO_COLOR").is_none();
        let (sender, receiver) = mpsc::unbounded();
        let thread = thread::spawn(move || {
            if terminal && io::stdout().is_terminal() {
                let receiver = Arc::new(Mutex::new(Some(receiver)));
                let closed = Arc::new(AtomicBool::new(false));
                let mut view = element!(LiveProgress(receiver:Some(receiver),color,limit,closed:Some(closed.clone())));
                let result = futures::executor::block_on(view.render_loop().output(Output::Stderr));
                if result.is_ok() && !closed.load(Ordering::Acquire) {
                    return interrupt_after_render();
                }
                result
            } else {
                futures::executor::block_on(plain_progress(receiver, limit))
            }
        });
        Self {
            sender: Some(sender),
            thread: Some(thread),
        }
    }

    fn send(&self, message: Message) -> io::Result<()> {
        if let Some(sender) = &self.sender {
            sender
                .unbounded_send(message)
                .map_err(|_| io::Error::other("progress output closed"))?;
        }
        Ok(())
    }

    pub fn stage(&self, action: &str, component: impl Into<String>) -> io::Result<()> {
        self.send(Message::Stage(format!("{action}: {}", component.into())))
    }
    pub fn runtime(&self, progress: &crate::runtime::run::RunProgress) -> io::Result<()> {
        if self.sender.is_none() {
            return Ok(());
        }
        match progress {
            crate::runtime::run::RunProgress::Preparing(progress) => {
                self.send(Message::Phase(progress.clone()))
            }
            crate::runtime::run::RunProgress::Scheduled(slots) => self.send(Message::Scheduled(
                slots.iter().map(ToString::to_string).collect(),
            )),
        }
    }
    pub fn event(&self, event: &RunEvent) -> io::Result<()> {
        if self.sender.is_none() {
            return Ok(());
        }
        // Profile outcomes can contain large scientific payloads; the display needs only metadata.
        if let RunEventKind::AttemptFinished { attempt } = &event.kind {
            return self.send(Message::Outcome {
                slot: attempt.resolved_slot_id.to_string(),
                status: attempt.status,
                diagnostic: attempt.diagnostic.clone(),
            });
        }
        self.send(Message::Event(event.clone()))
    }

    pub fn labels(&self, repository: &CheckedRepository, plan: &LogicalPlan) -> io::Result<()> {
        if self.sender.is_none() {
            return Ok(());
        }
        let mut labels = BTreeMap::new();
        for experiment in &plan.experiments {
            for candidate in &experiment.candidates {
                let dataset = repository
                    .planning_catalog
                    .dataset_definitions
                    .iter()
                    .find(|(_, id)| **id == candidate.dataset_configuration.record.definition)
                    .expect("checked dataset")
                    .0;
                let implementation = repository
                    .planning_catalog
                    .implementation_definitions
                    .iter()
                    .find(|(_, id)| {
                        **id == candidate.implementation_configuration.record.definition
                    })
                    .expect("checked implementation")
                    .0;
                for (repetition, specification) in candidate.specifications.iter().enumerate() {
                    for slot in &specification.attempt_slots {
                        let point = match slot.record.role {
                            crate::core::records::AttemptSlotRole::Warmup { warmup_index } => {
                                format!("warmup {}", warmup_index + 1)
                            }
                            crate::core::records::AttemptSlotRole::Measured {
                                observation_slot,
                            } => {
                                let measurement = specification
                                    .observation_slots
                                    .iter()
                                    .find(|point| point.id == observation_slot)
                                    .expect("planned observation")
                                    .record
                                    .measurement_index;
                                format!("measurement {}", measurement + 1)
                            }
                        };
                        labels.insert(
                            slot.id.to_string(),
                            Label {
                                text: format!(
                                    "{} / {dataset} / {implementation} / repeat {}, {point}",
                                    experiment.name,
                                    repetition + 1
                                ),
                                group: format!(
                                    "{implementation} / {dataset} ({})",
                                    experiment.name
                                ),
                            },
                        );
                    }
                }
            }
        }
        self.send(Message::Labels(labels))
    }

    fn stop(&mut self) -> io::Result<()> {
        if let Some(sender) = self.sender.take() {
            let _ = sender.unbounded_send(Message::Stop);
        }
        self.thread.take().map_or(Ok(()), |thread| {
            thread
                .join()
                .map_err(|_| io::Error::other("progress renderer panicked"))?
        })
    }
    pub fn finish(mut self) -> io::Result<()> {
        self.stop()
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct ProgressModel {
    terminal: bool,
    color: bool,
    start: Instant,
    last_line: Instant,
    phase: String,
    labels: BTreeMap<String, Label>,
    active: BTreeMap<String, Label>,
    total: usize,
    completed: usize,
    accepted: usize,
    failed: usize,
    censored: usize,
    superseded: usize,
    limit: Option<u64>,
    executing: bool,
    finished: bool,
    closed: bool,
    preparation_elapsed: Option<Duration>,
    failures: Vec<String>,
    benchmarks: BTreeMap<String, BenchmarkBar>,
    last_group: Option<String>,
}
impl ProgressModel {
    fn new(terminal: bool, color: bool, limit: Option<u64>) -> Self {
        Self {
            terminal,
            color,
            start: Instant::now(),
            last_line: Instant::now(),
            phase: String::new(),
            labels: BTreeMap::new(),
            active: BTreeMap::new(),
            total: 0,
            completed: 0,
            accepted: 0,
            failed: 0,
            censored: 0,
            superseded: 0,
            limit,
            executing: false,
            finished: false,
            closed: false,
            preparation_elapsed: None,
            failures: Vec::new(),
            benchmarks: BTreeMap::new(),
            last_group: None,
        }
    }
    fn line(&mut self, output: &mut impl Write, text: &str) -> io::Result<()> {
        if !self.terminal {
            let elapsed = self.start.elapsed().as_secs();
            writeln!(
                output,
                "[{}:{:02}] {}",
                elapsed / 60,
                elapsed % 60,
                clean(text)
            )?;
            output.flush()?;
        }
        self.last_line = Instant::now();
        Ok(())
    }
    fn status(&self) -> String {
        let elapsed = self.start.elapsed().as_secs();
        if !self.executing {
            return format!("[{}:{:02}] {}", elapsed / 60, elapsed % 60, self.phase);
        }
        format!(
            "Run {} | {}:{:02} elapsed | {} active | {} failed",
            bar(self.completed, self.total),
            elapsed / 60,
            elapsed % 60,
            self.active.len(),
            self.failed
        )
    }
    fn visible_bars(&self) -> Vec<String> {
        let mut groups = self
            .active
            .values()
            .map(|label| label.group.clone())
            .collect::<std::collections::BTreeSet<_>>();
        if groups.is_empty()
            && let Some(group) = &self.last_group
        {
            groups.insert(group.clone());
        }
        let mut ordered = groups.iter().cloned().collect::<Vec<_>>();
        ordered.extend(
            self.benchmarks
                .iter()
                .filter(|(name, b)| {
                    b.completed > 0 && b.completed < b.total && !groups.contains(*name)
                })
                .map(|(name, _)| name.clone()),
        );
        ordered
            .into_iter()
            .take(10)
            .filter_map(|group| {
                self.benchmarks.get(&group).map(|b| {
                    format!(
                        "{} {group}{}",
                        bar(b.completed, b.total),
                        if b.failed > 0 {
                            format!(" ({} failed)", b.failed)
                        } else {
                            String::new()
                        }
                    )
                })
            })
            .collect()
    }
    fn tick(&mut self, output: &mut impl Write) -> io::Result<()> {
        if !self.finished && self.last_line.elapsed() >= Duration::from_secs(10) {
            self.line(output, &self.status())?;
            for line in self.visible_bars() {
                self.line(output, &line)?;
            }
        }
        Ok(())
    }
    fn update(&mut self, output: &mut impl Write, message: Message) -> io::Result<()> {
        match message {
            Message::Scheduled(slots) => {
                self.benchmarks.clear();
                for slot in slots {
                    if let Some(label) = self.labels.get(&slot) {
                        self.benchmarks
                            .entry(label.group.clone())
                            .or_default()
                            .total += 1;
                    }
                }
                Ok(())
            }
            Message::Labels(labels) => {
                self.labels = labels;
                Ok(())
            }
            Message::Phase(progress) => {
                let action = match progress.phase {
                    PreparationPhase::VerifyingEnvironments => "Verifying environments",
                    PreparationPhase::ResolvingWorkers => "Resolving workers and staging sources",
                    PreparationPhase::PreparingDataset => {
                        "Preparing dataset (generate or verify cached data)"
                    }
                    PreparationPhase::PreparingProblem => "Preparing shared problem inputs",
                    PreparationPhase::CheckingApplicability => {
                        "Checking implementation applicability"
                    }
                    PreparationPhase::OpeningRunStore => {
                        "Opening run store and selecting pending work"
                    }
                };
                self.update(
                    output,
                    Message::Stage(format!("{action}: {}", progress.component)),
                )
            }
            Message::Stage(text) => {
                if self.phase == text {
                    return Ok(());
                }
                self.phase = text.clone();
                self.line(output, &text)
            }
            Message::Event(event) => match event.kind {
                RunEventKind::RunStarted { attempt_slots } => {
                    self.executing = true;
                    self.preparation_elapsed = Some(self.start.elapsed());
                    self.total = self.limit.map_or(attempt_slots, |limit| {
                        attempt_slots.min(usize::try_from(limit).unwrap_or(usize::MAX))
                    });
                    self.phase = "Executing benchmark".into();
                    self.line(output, &format!("Running {} pending attempts", self.total))
                }
                RunEventKind::AttemptStarted {
                    logical_slot_id,
                    resolved_slot_id,
                } => {
                    let label = self
                        .labels
                        .get(&logical_slot_id.to_string())
                        .cloned()
                        .unwrap_or_else(|| "selected benchmark attempt".into());
                    self.active
                        .insert(resolved_slot_id.to_string(), label.clone());
                    let active = self
                        .active
                        .values()
                        .filter(|other| other.group == label.group)
                        .count();
                    let b = self.benchmarks.entry(label.group.clone()).or_default();
                    b.total = b.total.max(b.completed + active);
                    self.phase = label.text.clone();
                    self.line(output, &format!("Starting {}", label.text))
                }
                RunEventKind::AttemptFinished { attempt } => self.update(
                    output,
                    Message::Outcome {
                        slot: attempt.resolved_slot_id.to_string(),
                        status: attempt.status,
                        diagnostic: attempt.diagnostic,
                    },
                ),
                RunEventKind::RunFinished {
                    accepted_attempts,
                    failed_attempts,
                    censored_attempts,
                    superseded_attempts,
                } => {
                    self.finished = true;
                    self.line(output, &self.status())?;
                    self.line(output,&format!("Finished: {accepted_attempts} accepted, {censored_attempts} censored, {failed_attempts} failed, {superseded_attempts} superseded"))
                }
            },
            Message::Outcome {
                slot,
                status: outcome,
                diagnostic,
            } => {
                let label = self
                    .active
                    .remove(&slot)
                    .unwrap_or_else(|| "selected benchmark attempt".into());
                self.completed += 1;
                let status = match outcome {
                    RunAttemptStatus::Accepted => {
                        self.accepted += 1;
                        "accepted"
                    }
                    RunAttemptStatus::Failed => {
                        self.failed += 1;
                        "failed"
                    }
                    RunAttemptStatus::Censored => {
                        self.censored += 1;
                        "censored"
                    }
                    RunAttemptStatus::Superseded => {
                        self.superseded += 1;
                        "superseded"
                    }
                };
                self.phase = self
                    .active
                    .values()
                    .next()
                    .map(|label| label.text.clone())
                    .unwrap_or_else(|| "Finalizing and scheduling".into());
                if let Some(d) = &diagnostic {
                    self.failures
                        .push(format!("{}: {} ({})", label.text, d.message, d.code));
                }
                let diagnostic = diagnostic
                    .as_ref()
                    .map(|d| format!(" ({}: {})", d.code, d.message))
                    .unwrap_or_default();
                let b = self.benchmarks.entry(label.group.clone()).or_default();
                b.completed += 1;
                if outcome == RunAttemptStatus::Failed {
                    b.failed += 1;
                }
                self.last_group = Some(label.group.clone());
                let benchmark_done = b.completed;
                let benchmark_total = b.total;
                self.line(
                    output,
                    &format!(
                        "{}/{} {status}: {}{diagnostic} {}",
                        self.completed,
                        self.total,
                        label.text,
                        bar(benchmark_done, benchmark_total)
                    ),
                )
            }
            Message::Tick => self.tick(output),
            Message::Stop => {
                self.closed = true;
                if !self.finished {
                    self.line(output, "Stopped")?;
                }
                Ok(())
            }
        }
    }
}
fn bar(done: usize, total: usize) -> String {
    let percent = if total == 0 {
        100
    } else {
        (done.min(total) as u128 * 100 / total as u128) as usize
    };
    let filled = percent * 12 / 100;
    format!(
        "[{}{}] {done}/{total} ({percent}%)",
        "=".repeat(filled),
        "-".repeat(12 - filled)
    )
}
fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn interrupt_after_render() -> io::Result<()> {
    // Let iocraft restore the terminal before preserving the CLI's interrupt behavior.
    #[cfg(unix)]
    {
        // SAFETY: SIGINT is a valid signal and raise has no pointer arguments.
        unsafe {
            libc::raise(libc::SIGINT);
        }
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "benchmark interrupted",
        ))
    }
    #[cfg(not(unix))]
    {
        std::process::exit(130);
    }
}

async fn receive(receiver: &mut UnboundedReceiver<Message>) -> Message {
    match futures::future::select(
        receiver.next(),
        futures_timer::Delay::new(Duration::from_millis(250)),
    )
    .await
    {
        futures::future::Either::Left((message, _)) => message.unwrap_or(Message::Stop),
        futures::future::Either::Right(_) => Message::Tick,
    }
}

async fn plain_progress(
    mut receiver: UnboundedReceiver<Message>,
    limit: Option<u64>,
) -> io::Result<()> {
    let mut model = ProgressModel::new(false, false, limit);
    let mut output = io::stderr().lock();
    loop {
        let message = receive(&mut receiver).await;
        let stop = matches!(message, Message::Stop);
        model.update(&mut output, message)?;
        if stop {
            return Ok(());
        }
    }
}

#[derive(Default, Props)]
struct LiveProgressProps {
    receiver: Option<Arc<Mutex<Option<UnboundedReceiver<Message>>>>>,
    color: bool,
    limit: Option<u64>,
    closed: Option<Arc<AtomicBool>>,
}

#[component]
fn LiveProgress(mut hooks: Hooks, props: &LiveProgressProps) -> impl Into<AnyElement<'static>> {
    let mut model = hooks.use_state(|| ProgressModel::new(true, props.color, props.limit));
    let receiver = props.receiver.clone().expect("live progress receiver");
    let closed = props.closed.clone().expect("live progress closure flag");
    hooks.use_future(async move {
        let mut receiver = receiver
            .lock()
            .expect("progress receiver lock")
            .take()
            .expect("progress receiver consumed once");
        loop {
            let message = receive(&mut receiver).await;
            let stop = matches!(message, Message::Stop);
            model
                .write()
                .update(&mut io::sink(), message)
                .expect("terminal model writes only to a sink");
            if stop {
                closed.store(true, Ordering::Release);
                break;
            }
        }
    });
    let model = model.read();
    if model.closed {
        hooks.use_context_mut::<SystemContext>().exit();
    }
    progress_view(&model)
}

fn progress_view(model: &ProgressModel) -> AnyElement<'static> {
    let prepared = model.preparation_elapsed.is_some();
    let stopped = model.closed && !model.finished;
    let elapsed = model.start.elapsed().as_secs();
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"]
        [((model.start.elapsed().as_millis() / 250) % 8) as usize];
    let color = |c| model.color.then_some(c);
    let mut children: Vec<AnyElement<'static>> = Vec::new();
    let preparation = if let Some(duration) = model.preparation_elapsed {
        format!("✓ Preparation complete · {:.1}s", duration.as_secs_f64())
    } else if stopped {
        "! Preparation stopped".into()
    } else {
        format!("{spinner} Preparation")
    };
    children.push(heading(
        preparation,
        color(if prepared { Color::Green } else { Color::Cyan }),
    ));
    if !prepared {
        children.push(detail(clean(&model.phase), None));
    }
    if model.executing {
        if model.finished || stopped {
            children.push(heading(
                format!(
                    "{} Benchmarks · {}/{} completed · {} accepted · {} failed · {} censored · {} superseded · {}:{:02}",
                    if stopped { "!" } else { "✓" }, model.completed, model.total,
                    model.accepted, model.failed, model.censored, model.superseded,
                    elapsed / 60, elapsed % 60,
                ),
                color(if model.failed > 0 { Color::Red } else { Color::Green }),
            ));
        } else {
            children.push(heading(
                format!(
                    "{spinner} Benchmarks · {} active · {}:{:02} elapsed",
                    model.active.len(),
                    elapsed / 60,
                    elapsed % 60
                ),
                color(Color::Cyan),
            ));
            children.push(element! {
                ProgressRow(label: "Overall", completed: model.completed, total: model.total, color: model.color)
            }.into());
            let active = model
                .active
                .values()
                .map(|label| label.group.clone())
                .collect::<std::collections::BTreeSet<_>>();
            let mut names = active.iter().cloned().collect::<Vec<_>>();
            names.extend(
                model
                    .benchmarks
                    .iter()
                    .filter(|(name, bar)| {
                        !active.contains(*name) && bar.completed > 0 && bar.completed < bar.total
                    })
                    .map(|(name, _)| name.clone()),
            );
            names.extend(
                model
                    .benchmarks
                    .iter()
                    .filter(|(name, bar)| {
                        !active.contains(*name) && bar.completed >= bar.total && bar.total > 0
                    })
                    .map(|(name, _)| name.clone()),
            );
            let hidden = names.len().saturating_sub(10);
            for name in names.into_iter().take(10) {
                if let Some(bar) = model.benchmarks.get(&name) {
                    if bar.completed >= bar.total {
                        children.push(detail(
                            format!(
                                "✓ {name} · {}/{} · {} failed",
                                bar.completed, bar.total, bar.failed
                            ),
                            color(if bar.failed > 0 {
                                Color::Red
                            } else {
                                Color::Green
                            }),
                        ));
                    } else {
                        children.push(element! {
                            ProgressRow(label: name, completed: bar.completed, total: bar.total, color: model.color)
                        }.into());
                    }
                }
            }
            if hidden > 0 {
                children.push(detail(
                    format!("{hidden} more benchmark groups · see final comparison"),
                    None,
                ));
            }
        }
    }
    children.extend(
        model.failures.iter().take(10).map(|message| {
            element!(Text(content:clean(message),color:color(Color::Red))).into_any()
        }),
    );
    if model.failures.len() > 10 {
        children.push(element!(Text(content:format!("{} additional failures retained in the run output",model.failures.len()-10))).into());
    }
    element! {
        View(flex_direction: FlexDirection::Column, width: 100pct, max_width: 100, padding_right: 2) {
            #(children)
        }
    }
    .into()
}

fn heading(content: String, color: Option<Color>) -> AnyElement<'static> {
    element!(Text(content, color, weight: Weight::Bold)).into()
}

fn detail(content: String, color: Option<Color>) -> AnyElement<'static> {
    element!(View(padding_left: 2) { Text(content, color) }).into()
}

#[derive(Default, Props)]
struct ProgressRowProps {
    label: String,
    completed: usize,
    total: usize,
    color: bool,
}

#[component]
fn ProgressRow(props: &ProgressRowProps) -> impl Into<AnyElement<'static>> {
    let percent = if props.total == 0 {
        100
    } else {
        (props.completed.min(props.total) as u128 * 100 / props.total as u128) as usize
    };
    element! {
        View(width: 100pct, gap: 1, align_items: AlignItems::Center) {
            View(width: 38pct, min_width: 0, padding_left: 2, flex_direction: FlexDirection::Column) {
                Text(content: clean(&props.label))
            }
            View(flex_grow: 1.0_f32, flex_basis: FlexBasis::Length(0), min_width: 3) {
                BarTrack(completed: props.completed, total: props.total, color: props.color)
            }
            View(width: 12, flex_shrink: 0.0_f32, flex_direction: FlexDirection::Column) {
                Text(content: format!("{}/{}", props.completed, props.total), align: TextAlign::Right)
            }
            View(width: 5, flex_shrink: 0.0_f32, flex_direction: FlexDirection::Column) {
                Text(content: format!("{percent}%"), align: TextAlign::Right)
            }
        }
    }
}

#[component]
fn BarTrack(mut hooks: Hooks, props: &ProgressRowProps) -> impl Into<AnyElement<'static>> {
    let width = hooks
        .use_component_rect()
        .map_or(12, |rect| (rect.right - rect.left).max(0) as usize)
        .saturating_sub(2);
    let filled = if props.total == 0 {
        width
    } else {
        (props.completed.min(props.total) as u128 * width as u128 / props.total as u128) as usize
    };
    let mut completed = MixedTextContent::new("█".repeat(filled));
    let mut remaining = MixedTextContent::new("░".repeat(width.saturating_sub(filled)));
    if props.color {
        completed.color = Some(if props.completed >= props.total {
            Color::Green
        } else {
            Color::Cyan
        });
        remaining.color = Some(Color::DarkGrey);
    }
    element! {
        View(width: 100pct, height: 1, overflow_x: Overflow::Hidden) {
            MixedText(
                contents: vec![MixedTextContent::new("["), completed, remaining, MixedTextContent::new("]")],
                wrap: TextWrap::NoWrap,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_sections_fold_and_failures_remain_visible() {
        let mut model = ProgressModel::new(true, false, None);
        model.phase = "Checking manifest: benchmark.yaml".into();
        let preparing = progress_view(&model).render(Some(80)).to_string();
        assert!(preparing.contains("Checking manifest"));
        model.executing = true;
        model.preparation_elapsed = Some(Duration::from_secs(2));
        model.total = 4;
        model.completed = 2;
        model.benchmarks.insert(
            "reference / numbers".into(),
            BenchmarkBar {
                total: 4,
                completed: 2,
                failed: 0,
            },
        );
        let active = progress_view(&model).render(Some(80)).to_string();
        assert!(active.contains("Preparation complete"), "{active}");
        assert!(!active.contains("Checking manifest"), "{active}");
        assert!(active.contains("reference / numbers"), "{active}");
        assert!(active.contains("50%"), "{active}");
        model.completed = 4;
        model.failed = 1;
        model.accepted = 3;
        model.finished = true;
        model
            .failures
            .push("reference failed: worker exited".into());
        let finished = progress_view(&model).render(Some(80)).to_string();
        assert!(finished.contains("4/4 completed"), "{finished}");
        assert!(!finished.contains("50%"), "{finished}");
        assert!(finished.contains("reference failed"), "{finished}");
        assert!(finished.lines().count() < 8, "{finished}");
    }
    #[test]
    fn progress_columns_align_for_different_labels_and_counts() {
        let mut view = element!(View(width:80,flex_direction:FlexDirection::Column) {
            ProgressRow(label:"β short",completed:1_usize,total:10_usize)
            ProgressRow(label:"longer solver name",completed:100_usize,total:100_usize)
        });
        let canvas = view.render(Some(80));
        assert_eq!(canvas.height(), 2);
        let percent_columns = (0..2)
            .map(|y| {
                (0..canvas.width())
                    .find(|&x| canvas.cell(x, y).and_then(|cell| cell.text()) == Some("%"))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(percent_columns[0], percent_columns[1]);
    }
    #[test]
    fn counters_and_labels_cover_concurrent_outcomes_and_invocation_bounds() {
        use crate::core::{
            records::RecordId,
            run_output::{RunAttemptOutput, RunDiagnostic},
        };
        let mut renderer = ProgressModel::new(false, false, Some(2));
        let mut output = Vec::new();
        let event = |kind| {
            Message::Event(RunEvent {
                command: "run",
                sequence: 0,
                kind,
            })
        };
        renderer
            .update(
                &mut output,
                event(RunEventKind::RunStarted { attempt_slots: 10 }),
            )
            .unwrap();
        let logical = RecordId::from_digest([0; 32]);
        let first = RecordId::from_digest([1; 32]);
        let second = RecordId::from_digest([2; 32]);
        renderer
            .labels
            .insert(format!("{logical}"), "main / numbers / reference".into());
        for slot in [first, second] {
            renderer
                .update(
                    &mut output,
                    event(RunEventKind::AttemptStarted {
                        logical_slot_id: logical,
                        resolved_slot_id: slot,
                    }),
                )
                .unwrap();
        }
        assert_eq!(renderer.active.len(), 2);
        for (slot, status) in [
            (first, RunAttemptStatus::Accepted),
            (second, RunAttemptStatus::Failed),
        ] {
            renderer
                .update(
                    &mut output,
                    event(RunEventKind::AttemptFinished {
                        attempt: Box::new(RunAttemptOutput {
                            observations: vec![],
                            logical_slot_id: logical,
                            resolved_slot_id: slot,
                            attempt_id: RecordId::from_digest([0; 32]),
                            status,
                            observation_id: None,
                            attempt_directory: String::new(),
                            observation_directory: None,
                            result: None,
                            metrics: None,
                            diagnostic: (status == RunAttemptStatus::Failed).then(|| {
                                RunDiagnostic {
                                    code: "worker_failed".into(),
                                    message: "worker exited".into(),
                                }
                            }),
                        }),
                    }),
                )
                .unwrap();
        }
        assert_eq!(renderer.active.len(), 0);
        assert_eq!(
            (
                renderer.completed,
                renderer.accepted,
                renderer.failed,
                renderer.total
            ),
            (2, 1, 1, 2)
        );
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("2/2 failed: main / numbers / reference"));
        assert!(text.contains("worker_failed: worker exited"));
        assert!(!text.contains("mw1-"));
    }
    #[test]
    fn benchmark_bars_use_pending_work_for_multiple_groups() {
        let mut renderer = ProgressModel::new(true, false, None);
        renderer.labels.insert(
            "first".into(),
            Label {
                text: "first".into(),
                group: "reference / numbers".into(),
            },
        );
        renderer.labels.insert(
            "second".into(),
            Label {
                text: "second".into(),
                group: "reference / numbers".into(),
            },
        );
        renderer.labels.insert(
            "other".into(),
            Label {
                text: "other".into(),
                group: "packed / numbers".into(),
            },
        );
        let mut output = Vec::new();
        renderer
            .update(
                &mut output,
                Message::Scheduled(vec!["first".into(), "second".into(), "other".into()]),
            )
            .unwrap();
        assert_eq!(renderer.benchmarks["reference / numbers"].total, 2);
        assert_eq!(renderer.benchmarks["packed / numbers"].total, 1);
        renderer.executing = true;
        renderer.total = 3;
        renderer
            .active
            .insert("active".into(), renderer.labels["first"].clone());
        renderer
            .active
            .insert("other-active".into(), renderer.labels["other"].clone());
        let text = progress_view(&renderer).render(Some(80)).to_string();
        for expected in [
            "Overall",
            "0/3",
            "0/2",
            "0/1",
            "reference / numbers",
            "packed / numbers",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        assert_eq!(bar(1, 2), "[======------] 1/2 (50%)");
        assert_eq!(bar(2, 2), "[============] 2/2 (100%)");
    }
    #[test]
    fn redirected_long_tasks_get_heartbeats() {
        let mut renderer = ProgressModel::new(false, false, None);
        renderer.phase = "Preparing dataset".into();
        renderer.last_line = Instant::now() - Duration::from_secs(11);
        let mut output = Vec::new();
        renderer.tick(&mut output).unwrap();
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("Preparing dataset")
        );
    }
}
