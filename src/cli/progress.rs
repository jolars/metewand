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
    preparation_steps: Vec<String>,
    failures: Vec<String>,
    benchmarks: BTreeMap<String, BenchmarkBar>,
    group_order: Vec<String>,
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
            preparation_steps: Vec::new(),
            failures: Vec::new(),
            benchmarks: BTreeMap::new(),
            group_order: Vec::new(),
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
    fn visible_groups(&self) -> impl Iterator<Item = (&str, &BenchmarkBar)> {
        self.group_order
            .iter()
            .take(10)
            .filter_map(|name| self.benchmarks.get(name).map(|bar| (name.as_str(), bar)))
    }
    fn visible_bars(&self) -> Vec<String> {
        self.visible_groups()
            .map(|(group, b)| {
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
            .collect()
    }
    fn tick(&mut self, output: &mut impl Write) -> io::Result<()> {
        if !self.finished && self.last_line.elapsed() >= Duration::from_secs(10) {
            for line in self.visible_bars() {
                self.line(output, &line)?;
            }
            self.line(output, &self.status())?;
        }
        Ok(())
    }
    fn update(&mut self, output: &mut impl Write, message: Message) -> io::Result<()> {
        match message {
            Message::Scheduled(slots) => {
                self.benchmarks.clear();
                self.group_order.clear();
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
                if !self.executing {
                    // Preparation is sequential, so the next step completes the preceding one.
                    self.preparation_steps.push(text.clone());
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
                    if !self.group_order.contains(&label.group) {
                        self.group_order.push(label.group.clone());
                    }
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
                RunEventKind::RunFinished { .. } => {
                    self.finished = true;
                    Ok(())
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
    let counter_digits = model
        .benchmarks
        .values()
        .fold(model.total.max(model.completed), |largest, bar| {
            largest.max(bar.total).max(bar.completed)
        })
        .to_string()
        .len()
        .max(2);
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
        color(if prepared {
            Color::Green
        } else if stopped {
            Color::Yellow
        } else {
            Color::Cyan
        }),
    ));
    if !prepared {
        if model.preparation_steps.is_empty() {
            children.push(detail(clean(&model.phase), None));
        }
        for (index, step) in model.preparation_steps.iter().enumerate() {
            let complete = index + 1 < model.preparation_steps.len();
            let marker = if complete {
                "✓"
            } else if stopped {
                "!"
            } else {
                spinner
            };
            let text_color = if complete {
                color(Color::DarkGrey)
            } else if stopped {
                color(Color::Yellow)
            } else {
                None
            };
            let marker_color = if complete || stopped {
                text_color
            } else {
                color(Color::Cyan)
            };
            children.push(
                element! {
                    MarkedLabel(label: step.clone(), marker, color: text_color, marker_color)
                }
                .into(),
            );
        }
    }
    if model.executing && !model.finished {
        if stopped {
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
            let hidden = model.group_order.len().saturating_sub(10);
            for (name, bar) in model.visible_groups() {
                children.push(element! {
                    ProgressRow(label: name, completed: bar.completed, total: bar.total, failed: bar.failed,
                        spinner,
                        counter_digits, color: model.color)
                }.into());
            }
            if hidden > 0 {
                children.push(detail(
                    format!("{hidden} more benchmark groups · inspect with status"),
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
    if model.executing && !model.finished && !stopped {
        children.push(
            element! {
                ProgressRow(label: "Overall", completed: model.completed, total: model.total,
                    counter_digits, emphasized: true, color: model.color)
            }
            .into(),
        );
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
struct MarkedLabelProps {
    label: String,
    marker: String,
    color: Option<Color>,
    marker_color: Option<Color>,
    weight: Weight,
}

#[component]
fn MarkedLabel(props: &MarkedLabelProps) -> impl Into<AnyElement<'static>> {
    element! {
        View(width: 100pct, min_width: 0, padding_left: 2, gap: 1) {
            View(width: 1, flex_shrink: 0.0_f32) {
                Text(content: &props.marker, color: props.marker_color)
            }
            View(flex_grow: 1.0_f32, flex_basis: FlexBasis::Length(0), min_width: 0) {
                Text(content: clean(&props.label), weight: props.weight, color: props.color)
            }
        }
    }
}

#[derive(Default, Props)]
struct ProgressRowProps {
    label: String,
    completed: usize,
    total: usize,
    color: bool,
    failed: usize,
    counter_digits: usize,
    emphasized: bool,
    spinner: String,
}

#[component]
fn ProgressRow(props: &ProgressRowProps) -> impl Into<AnyElement<'static>> {
    let complete = props.total > 0 && props.completed >= props.total && !props.emphasized;
    let weight = if props.emphasized {
        Weight::Bold
    } else {
        Weight::Normal
    };
    let row_color = if complete {
        props.color.then_some(Color::DarkGrey)
    } else if props.emphasized {
        props.color.then_some(Color::Cyan)
    } else {
        None
    };
    let marker = if complete {
        if props.failed > 0 { "✗" } else { "✓" }
    } else {
        &props.spinner
    };
    let marker_color = if complete {
        row_color
    } else {
        props.color.then_some(Color::Cyan)
    };
    let digits = props
        .counter_digits
        .max(2)
        .max(props.completed.to_string().len())
        .max(props.total.to_string().len());
    let mut separator = MixedTextContent::new("/");
    separator.color = props.color.then_some(Color::DarkGrey);
    let mut completed_count =
        MixedTextContent::new(format!("{:>digits$}", props.completed)).weight(weight);
    let mut total_count = MixedTextContent::new(format!("{:>digits$}", props.total)).weight(weight);
    completed_count.color = row_color;
    total_count.color = row_color;
    let percent = if props.total == 0 {
        100
    } else {
        (props.completed.min(props.total) as u128 * 100 / props.total as u128) as usize
    };
    element! {
        View(width: 100pct, gap: 1, align_items: AlignItems::Center) {
            View(width: 38pct, min_width: 0, flex_direction: FlexDirection::Column) {
                MarkedLabel(label: props.label.clone(), marker, weight, color: row_color, marker_color)
            }
            View(flex_grow: 1.0_f32, flex_basis: FlexBasis::Length(0), min_width: 3) {
                #(if complete {
                    element!(Text(content: if props.failed > 0 { format!("{} failed", props.failed) } else { "complete".into() },
                        color: row_color)).into_any()
                } else {
                    element!(BarTrack(completed: props.completed, total: props.total, color: props.color)).into_any()
                })
            }
            View(width: (digits * 2 + 1).max(12) as u32, flex_shrink: 0.0_f32, flex_direction: FlexDirection::Column) {
                MixedText(contents: vec![
                    completed_count,
                    separator,
                    total_count,
                ], align: TextAlign::Right, wrap: TextWrap::NoWrap)
            }
            View(width: 5, flex_shrink: 0.0_f32, flex_direction: FlexDirection::Column) {
                Text(content: format!("{percent}%"), align: TextAlign::Right, weight, color: row_color)
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
    fn preparation_rows_accumulate_then_fold_when_execution_starts() {
        for color in [true, false] {
            let mut model = ProgressModel::new(true, color, None);
            for step in [
                "Checking manifest: demo.yaml",
                "Checking manifest: demo.yaml",
                "Preparing dataset: small",
            ] {
                model
                    .update(&mut io::sink(), Message::Stage(step.into()))
                    .unwrap();
            }
            let canvas = progress_view(&model).render(Some(100));
            let text = canvas.to_string();
            assert_eq!(text.matches("Checking manifest").count(), 1, "{text}");
            let completed_y = text
                .lines()
                .position(|line| line.contains("Checking manifest"))
                .unwrap();
            let active_y = text
                .lines()
                .position(|line| line.contains("Preparing dataset"))
                .unwrap();
            assert!(completed_y < active_y, "{text}");
            assert_eq!(
                canvas.cell(2, completed_y).and_then(|cell| cell.text()),
                Some("✓")
            );
            let marker = canvas
                .cell(2, active_y)
                .and_then(|cell| cell.text())
                .unwrap();
            assert!("⠋⠙⠹⠸⠼⠴⠦⠧".contains(marker), "{text}");
            assert_eq!(
                canvas
                    .cell(4, completed_y)
                    .unwrap()
                    .text_style()
                    .unwrap()
                    .color,
                color.then_some(Color::DarkGrey)
            );
            model
                .update(
                    &mut io::sink(),
                    Message::Event(RunEvent {
                        command: "run",
                        sequence: 0,
                        kind: RunEventKind::RunStarted { attempt_slots: 1 },
                    }),
                )
                .unwrap();
            let folded = progress_view(&model).render(Some(100)).to_string();
            assert!(folded.contains("Preparation complete"), "{folded}");
            assert!(
                !folded.contains("Checking manifest") && !folded.contains("Preparing dataset"),
                "{folded}"
            );
        }
    }

    #[test]
    fn stopped_preparation_preserves_the_unfinished_step() {
        let mut model = ProgressModel::new(true, false, None);
        model
            .update(
                &mut io::sink(),
                Message::Stage("Checking manifest: demo.yaml".into()),
            )
            .unwrap();
        model
            .update(
                &mut io::sink(),
                Message::Stage("Preparing dataset: broken".into()),
            )
            .unwrap();
        model.update(&mut io::sink(), Message::Stop).unwrap();
        let text = progress_view(&model).render(Some(100)).to_string();
        assert!(text.contains("✓ Checking manifest: demo.yaml"), "{text}");
        assert!(text.contains("! Preparing dataset: broken"), "{text}");
        assert!(text.contains("Preparation stopped"), "{text}");
        assert!(!text.contains("Preparation complete"), "{text}");
    }

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
        model.group_order.push("reference / numbers".into());
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
        assert!(!finished.contains("Benchmarks"), "{finished}");
        assert!(!finished.contains("50%"), "{finished}");
        assert!(finished.contains("reference failed"), "{finished}");
        assert!(finished.lines().count() < 8, "{finished}");
    }
    #[test]
    fn overall_progress_follows_details_and_is_emphasized() {
        let mut model = ProgressModel::new(true, false, None);
        model.executing = true;
        model.preparation_elapsed = Some(Duration::from_secs(1));
        model.total = 10;
        model.completed = 2;
        model.benchmarks.insert(
            "reference / numbers".into(),
            BenchmarkBar {
                total: 10,
                completed: 2,
                failed: 0,
            },
        );
        model.group_order.push("reference / numbers".into());
        for width in [80, 110] {
            let canvas = progress_view(&model).render(Some(width));
            let text = canvas.to_string();
            assert!(
                text.find("Overall").unwrap() > text.find("reference / numbers").unwrap(),
                "{text}"
            );
            let y = text
                .lines()
                .position(|line| line.contains("Overall"))
                .unwrap();
            let x = (0..canvas.width())
                .find(|&x| canvas.cell(x, y).and_then(|cell| cell.text()) == Some("O"))
                .unwrap();
            assert_eq!(
                canvas.cell(x, y).unwrap().text_style().unwrap().weight,
                Weight::Bold,
                "{text}"
            );
        }
    }

    #[test]
    fn rows_appear_on_first_start_and_keep_their_order_and_spinners() {
        use crate::core::records::RecordId;
        let mut model = ProgressModel::new(true, false, None);
        model.executing = true;
        model.preparation_elapsed = Some(Duration::from_secs(1));
        model.total = 6;
        for name in ["alpha", "beta", "gamma"] {
            model.benchmarks.insert(
                name.into(),
                BenchmarkBar {
                    total: 2,
                    ..Default::default()
                },
            );
        }
        let start = |model: &mut ProgressModel, name: &str, index| {
            let id = RecordId::from_digest([index; 32]);
            let resolved = RecordId::from_digest([index; 32]);
            model.labels.insert(id.to_string(), name.into());
            model
                .update(
                    &mut io::sink(),
                    Message::Event(RunEvent {
                        command: "run",
                        sequence: 0,
                        kind: RunEventKind::AttemptStarted {
                            logical_slot_id: id,
                            resolved_slot_id: resolved,
                        },
                    }),
                )
                .unwrap();
            resolved.to_string()
        };
        let render = |model: &ProgressModel| progress_view(model).render(Some(80)).to_string();
        let initial = render(&model);
        assert!(
            !initial.contains("alpha") && !initial.contains("beta") && !initial.contains("gamma"),
            "{initial}"
        );
        let first = start(&mut model, "beta", 1);
        let running = render(&model);
        assert!(
            running.contains("beta") && !running.contains("alpha") && !running.contains("gamma"),
            "{running}"
        );
        model
            .update(
                &mut io::sink(),
                Message::Outcome {
                    slot: first,
                    status: RunAttemptStatus::Accepted,
                    diagnostic: None,
                },
            )
            .unwrap();
        assert!(model.active.is_empty());
        let between = render(&model);
        let beta = between.lines().find(|line| line.contains("beta")).unwrap();
        assert!(
            "⠋⠙⠹⠸⠼⠴⠦⠧".contains(beta.trim_start().chars().next().unwrap()),
            "{between}"
        );
        start(&mut model, "alpha", 2);
        let before_gamma = render(&model);
        assert!(
            before_gamma.find("beta").unwrap() < before_gamma.find("alpha").unwrap(),
            "{before_gamma}"
        );
        assert!(!before_gamma.contains("gamma"), "{before_gamma}");
        start(&mut model, "gamma", 3);
        let positions = |text: &str| {
            ["beta", "alpha", "gamma"]
                .map(|name| text.lines().position(|line| line.contains(name)).unwrap())
        };
        let initial_positions = positions(&render(&model));
        assert!(initial_positions.windows(2).all(|pair| pair[0] < pair[1]));
        let second = start(&mut model, "beta", 4);
        model
            .update(
                &mut io::sink(),
                Message::Outcome {
                    slot: second,
                    status: RunAttemptStatus::Accepted,
                    diagnostic: None,
                },
            )
            .unwrap();
        let final_text = render(&model);
        assert_eq!(positions(&final_text), initial_positions);
        assert_eq!(
            final_text
                .lines()
                .filter(|line| line.contains("beta"))
                .count(),
            1
        );
        assert_eq!(model.active.len(), 2);
    }

    #[test]
    fn row_markers_align_and_completed_rows_are_muted() {
        let mut model = ProgressModel::new(true, true, None);
        model.executing = true;
        model.preparation_elapsed = Some(Duration::from_secs(1));
        model.total = 12;
        model.completed = 4;
        for (name, completed) in [("Active", 0), ("Completed", 4), ("Pending", 0)] {
            model.benchmarks.insert(
                name.into(),
                BenchmarkBar {
                    total: 4,
                    completed,
                    failed: 0,
                },
            );
        }
        model.active.insert("running".into(), "Active".into());
        model.group_order = vec!["Active".into(), "Completed".into()];
        for color in [true, false] {
            model.color = color;
            let canvas = progress_view(&model).render(Some(80));
            let text = canvas.to_string();
            assert!(!text.contains("Pending"), "{text}");
            let positions = ["Active", "Completed"].map(|name| {
                let y = text
                    .lines()
                    .position(|line| line.contains(name))
                    .unwrap_or_else(|| panic!("missing {name}: {text}"));
                let x = (0..canvas.width())
                    .find(|&x| canvas.cell(x, y).and_then(|cell| cell.text()) == Some(&name[..1]))
                    .unwrap();
                (x, y)
            });
            assert!(
                positions.windows(2).all(|pair| pair[0].0 == pair[1].0),
                "{text}"
            );
            let marker_x = positions[0].0 - 2;
            let spinner = canvas
                .cell(marker_x, positions[0].1)
                .and_then(|cell| cell.text())
                .unwrap();
            assert!("⠋⠙⠹⠸⠼⠴⠦⠧".contains(spinner), "{text}");
            assert_eq!(
                canvas
                    .cell(marker_x, positions[1].1)
                    .and_then(|cell| cell.text()),
                Some("✓")
            );
            for x in 0..canvas.width() {
                let cell = canvas.cell(x, positions[1].1).unwrap();
                if cell.text().is_some_and(|text| !text.trim().is_empty()) {
                    assert_eq!(
                        cell.text_style().unwrap().color,
                        color.then_some(Color::DarkGrey),
                        "{text}"
                    );
                }
            }
        }
    }

    #[test]
    fn progress_columns_align_for_different_labels_and_counts() {
        let mut view = element!(View(width:80,flex_direction:FlexDirection::Column) {
            ProgressRow(label:"β short",completed:1_usize,total:10_usize)
            ProgressRow(label:"longer solver name",completed:10_usize,total:98_usize)
            ProgressRow(label:"small dataset",completed:1_usize,total:4_usize)
            ProgressRow(label:"finished solver",completed:4_usize,total:4_usize)
        });
        let canvas = view.render(Some(80));
        assert_eq!(canvas.height(), 4);
        let text = canvas.to_string();
        for expected in [" 1/10", "10/98", " 1/ 4"] {
            assert!(text.contains(expected), "{text}");
        }
        let slash_columns = (0..4)
            .map(|y| {
                (0..canvas.width())
                    .find(|&x| canvas.cell(x, y).and_then(|cell| cell.text()) == Some("/"))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(
            slash_columns
                .windows(2)
                .all(|columns| columns[0] == columns[1]),
            "{text}"
        );
        let percent_columns = (0..4)
            .map(|y| {
                (0..canvas.width())
                    .find(|&x| canvas.cell(x, y).and_then(|cell| cell.text()) == Some("%"))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(
            percent_columns
                .windows(2)
                .all(|columns| columns[0] == columns[1])
        );
    }

    #[test]
    fn progress_count_separator_is_muted_only_with_color() {
        for color in [true, false] {
            let mut view =
                element!(ProgressRow(label:"reference",completed:1_usize,total:4_usize,color));
            let canvas = view.render(Some(80));
            let x = (0..canvas.width())
                .find(|&x| canvas.cell(x, 0).and_then(|cell| cell.text()) == Some("/"))
                .unwrap();
            assert_eq!(
                canvas.cell(x, 0).unwrap().text_style().unwrap().color,
                color.then_some(Color::DarkGrey)
            );
        }
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
        renderer.group_order = vec!["reference / numbers".into(), "packed / numbers".into()];
        let text = progress_view(&renderer).render(Some(80)).to_string();
        for expected in [
            "Overall",
            "0/ 3",
            "0/ 2",
            "0/ 1",
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
