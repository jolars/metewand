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
use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(super) struct Progress {
    sender: Option<Sender<Message>>,
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
        let (sender, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut renderer = Renderer::new(terminal, color, limit);
            let mut output = io::stderr().lock();
            loop {
                match receiver.recv_timeout(Duration::from_secs(1)) {
                    Ok(Message::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        renderer.clear(&mut output)?;
                        if !renderer.finished {
                            renderer.line(&mut output, "Stopped", 33)?;
                        }
                        return output.flush();
                    }
                    Ok(message) => {
                        renderer.update(&mut output, message)?;
                        if renderer.terminal {
                            renderer.tick(&mut output)?;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => renderer.tick(&mut output)?,
                }
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
                .send(message)
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
            let _ = sender.send(Message::Stop);
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

struct Renderer {
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
    drawn: usize,
    benchmarks: BTreeMap<String, BenchmarkBar>,
    last_group: Option<String>,
}
impl Renderer {
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
            drawn: 0,
            benchmarks: BTreeMap::new(),
            last_group: None,
        }
    }
    fn clear(&mut self, output: &mut impl Write) -> io::Result<()> {
        for row in 0..self.drawn {
            write!(output, "\r\x1b[2K")?;
            if row + 1 < self.drawn {
                write!(output, "\x1b[1A")?;
            }
        }
        self.drawn = 0;
        Ok(())
    }
    fn line(&mut self, output: &mut impl Write, text: &str, color: u8) -> io::Result<()> {
        self.clear(output)?;
        let elapsed = self.start.elapsed().as_secs();
        let text = clean(text);
        if self.color {
            write!(output, "\x1b[{color}m")?;
        }
        write!(output, "[{}:{:02}] {text}", elapsed / 60, elapsed % 60)?;
        if self.color {
            write!(output, "\x1b[0m")?;
        }
        writeln!(output)?;
        self.last_line = Instant::now();
        output.flush()
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
            .take(terminal_height().saturating_sub(4).max(1))
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
        if self.finished {
            return Ok(());
        }
        if self.terminal {
            self.clear(output)?;
            let mut lines = vec![self.status()];
            if self.executing {
                lines.extend(self.visible_bars());
            }
            for (row, text) in lines.iter().enumerate() {
                if row > 0 {
                    writeln!(output)?;
                }
                if self.color {
                    write!(output, "\x1b[36m")?;
                }
                write!(
                    output,
                    "\r{}",
                    fit(text, terminal_width().saturating_sub(2))
                )?;
                if self.color {
                    write!(output, "\x1b[0m")?;
                }
            }
            self.drawn = lines.len();
            output.flush()
        } else if self.last_line.elapsed() >= Duration::from_secs(10) {
            self.line(output, &self.status(), 36)?;
            for line in self.visible_bars() {
                self.line(output, &line, 36)?;
            }
            Ok(())
        } else {
            Ok(())
        }
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
                self.line(output, &text, 36)
            }
            Message::Event(event) => match event.kind {
                RunEventKind::RunStarted { attempt_slots } => {
                    self.executing = true;
                    self.total = self.limit.map_or(attempt_slots, |limit| {
                        attempt_slots.min(usize::try_from(limit).unwrap_or(usize::MAX))
                    });
                    self.phase = "Executing benchmark".into();
                    self.line(
                        output,
                        &format!("Running {} pending attempts", self.total),
                        36,
                    )
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
                    if !self.terminal || (b.completed == 0 && active == 1) {
                        self.line(output, &format!("Starting {}", label.text), 36)
                    } else {
                        Ok(())
                    }
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
                    self.line(
                        output,
                        &self.status(),
                        if failed_attempts > 0 { 31 } else { 32 },
                    )?;
                    self.line(output,&format!("Finished: {accepted_attempts} accepted, {censored_attempts} censored, {failed_attempts} failed, {superseded_attempts} superseded"),if failed_attempts>0 {31} else {32})
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
                let (status, color) = match outcome {
                    RunAttemptStatus::Accepted => {
                        self.accepted += 1;
                        ("accepted", 32)
                    }
                    RunAttemptStatus::Failed => {
                        self.failed += 1;
                        ("failed", 31)
                    }
                    RunAttemptStatus::Censored => {
                        self.censored += 1;
                        ("censored", 33)
                    }
                    RunAttemptStatus::Superseded => {
                        self.superseded += 1;
                        ("superseded", 33)
                    }
                };
                self.phase = self
                    .active
                    .values()
                    .next()
                    .map(|label| label.text.clone())
                    .unwrap_or_else(|| "Finalizing and scheduling".into());
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
                let benchmark_failed = b.failed;
                if self.terminal && outcome != RunAttemptStatus::Failed {
                    if benchmark_done >= benchmark_total {
                        let failures = if benchmark_failed > 0 {
                            format!(", {benchmark_failed} failed")
                        } else {
                            String::new()
                        };
                        self.line(
                            output,
                            &format!(
                                "{} {} — completed{failures}",
                                bar(benchmark_done, benchmark_total),
                                label.group
                            ),
                            if benchmark_failed > 0 { 31 } else { color },
                        )
                    } else {
                        Ok(())
                    }
                } else {
                    self.line(
                        output,
                        &format!(
                            "{}/{} {status}: {}{diagnostic} {}",
                            self.completed,
                            self.total,
                            label.text,
                            bar(benchmark_done, benchmark_total)
                        ),
                        color,
                    )
                }
            }
            Message::Stop => Ok(()),
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
fn fit(text: &str, width: usize) -> String {
    let mut used = 0;
    clean(text)
        .chars()
        .take_while(|c| {
            used += if c.is_ascii() { 1 } else { 2 };
            used <= width
        })
        .collect()
}
fn terminal_height() -> usize {
    #[cfg(unix)]
    {
        let mut size = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: ioctl writes a winsize into the valid pointer for this request.
        if unsafe { libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &raw mut size) } == 0
            && size.ws_row > 0
        {
            return usize::from(size.ws_row);
        }
    }
    24
}

fn terminal_width() -> usize {
    #[cfg(unix)]
    {
        let mut size = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: ioctl writes a winsize into the valid pointer for this request.
        if unsafe { libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &raw mut size) } == 0
            && size.ws_col > 0
        {
            return usize::from(size.ws_col);
        }
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|width| *width > 0)
        .unwrap_or(80)
}

fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_terminal_redraws_and_plain_logs_avoid_escape_sequences() {
        for (terminal, color) in [(true, true), (true, false), (false, false)] {
            let mut renderer = Renderer::new(terminal, color, None);
            let mut output = Vec::new();
            renderer
                .update(
                    &mut output,
                    Message::Stage("Checking manifest: benchmark".into()),
                )
                .unwrap();
            renderer.tick(&mut output).unwrap();
            renderer.clear(&mut output).unwrap();
            let text = String::from_utf8(output).unwrap();
            assert_eq!(text.contains('\r'), terminal);
            assert_eq!(text.contains("\x1b[36m"), color);
            if !terminal {
                assert!(!text.contains('\x1b'));
            }
        }
    }
    #[test]
    fn counters_and_labels_cover_concurrent_outcomes_and_invocation_bounds() {
        use crate::core::{
            records::RecordId,
            run_output::{RunAttemptOutput, RunDiagnostic},
        };
        let mut renderer = Renderer::new(false, false, Some(2));
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
    fn benchmark_bars_use_pending_work_and_refresh_multiple_rows() {
        let mut renderer = Renderer::new(true, false, None);
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
        renderer.tick(&mut output).unwrap();
        renderer.tick(&mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Run [------------] 0/3 (0%)"));
        assert!(text.contains("[------------] 0/2 (0%) reference / numbers"));
        assert!(text.contains("[------------] 0/1 (0%) packed / numbers"));
        assert!(text.contains("\x1b[1A"));
        assert_eq!(bar(1, 2), "[======------] 1/2 (50%)");
        assert_eq!(bar(2, 2), "[============] 2/2 (100%)");
    }
    #[test]
    fn redirected_long_tasks_get_heartbeats() {
        let mut renderer = Renderer::new(false, false, None);
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
