//! Private crash points, armed explicitly by subprocess tests only.

use std::{cell::RefCell, fs::File, io::Write};

use rusqlite::{
    Connection,
    trace::{TraceEvent, TraceEventCodes},
};

struct Session {
    trace: File,
    target: usize,
    count: usize,
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

pub(crate) fn arm(trace: File, target: usize) {
    SESSION.with_borrow_mut(|session| {
        *session = Some(Session {
            trace,
            target,
            count: 0,
        })
    });
}

pub(crate) fn hit(label: &str) {
    SESSION.with_borrow_mut(|session| {
        if let Some(session) = session {
            session.count += 1;
            writeln!(session.trace, "{label}").unwrap();
            if session.count == session.target {
                // Bypass destructors, SQLite rollback, and temporary-directory
                // cleanup, as an abruptly terminated worker would.
                std::process::exit(86);
            }
        }
    });
}

pub(crate) fn trace_database(connection: &Connection) {
    connection.trace_v2(
        TraceEventCodes::SQLITE_TRACE_STMT | TraceEventCodes::SQLITE_TRACE_PROFILE,
        Some(|event| {
            let (phase, sql) = match event {
                // SQLite also emits annotations for trigger subprograms. Keep
                // actual statements, including SQL with leading comments.
                TraceEvent::Stmt(statement, sql) if sql == statement.sql().as_ref() => {
                    ("before", statement.sql().into_owned())
                }
                TraceEvent::Profile(statement, _) => ("after", statement.sql().into_owned()),
                _ => return,
            };
            let statement = sql
                .lines()
                .filter(|line| !line.trim_start().starts_with("--"))
                .flat_map(str::split_whitespace)
                .take(8)
                .collect::<Vec<_>>()
                .join(" ");
            hit(&format!("database.{phase} {statement}"));
        }),
    );
}
