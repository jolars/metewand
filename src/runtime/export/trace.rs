use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{
    core::{export::metric_row_count, profile::TracePoint},
    runtime::evidence::{TracePosition, TraceReader, buffered_trace_path},
};

use super::ExportError;

/// Enough information to replay a trace in the v1 bytewise ID order.
pub(super) struct Trace {
    path: PathBuf,
    starts: Vec<TracePosition>,
    pub(super) len: usize,
    pub(super) rows: usize,
}

impl Trace {
    pub(super) fn read(
        root: &Path,
        attempt_id: &str,
        profile: &Value,
    ) -> Result<Option<Self>, ExportError> {
        let Some(path) = buffered_trace_path(root, profile)? else {
            return Ok(None);
        };
        let mut reader = TraceReader::open(&path, attempt_id, TracePosition::default())?;
        let mut starts = Vec::new();
        let mut next_start = 0_usize;
        let mut len = 0;
        let mut rows = 0;
        while let Some((position, point)) = reader.next_point()? {
            if len == next_start {
                starts.push(position);
                next_start = next_start.saturating_mul(10).max(10);
            }
            len += 1;
            rows += metric_row_count(&point.provisional_metrics);
        }
        Ok(Some(Self {
            path,
            starts,
            len,
            rows,
        }))
    }

    pub(super) fn visit(
        &self,
        attempt_id: &str,
        mut visit: impl FnMut(usize, TracePoint) -> Result<(), ExportError>,
    ) -> Result<(), ExportError> {
        // Within each decimal width, lexicographic order is also file order.
        // Keep one sequential reader per width so no point-sized index is needed.
        let mut ranges = self
            .starts
            .iter()
            .map(|&start| TraceReader::open(&self.path, attempt_id, start))
            .collect::<Result<Vec<_>, _>>()?;
        for index in indices(self.len) {
            let width = index.checked_ilog10().unwrap_or(0) as usize;
            let (_, point) = ranges[width]
                .next_point()?
                .ok_or(ExportError::InvalidEvidence(
                    "buffered trace ended during export",
                ))?;
            visit(index, point)?;
        }
        Ok(())
    }
}

/// Visit each nonzero decimal prefix before its children, starting with zero.
fn indices(len: usize) -> impl Iterator<Item = usize> {
    (0..len.min(1)).chain(std::iter::successors(
        (len > 1).then_some(1),
        move |&index| {
            if index <= (len - 1) / 10 {
                return Some(index * 10);
            }
            let mut index = index;
            while index % 10 == 9 || index + 1 == len {
                index /= 10;
            }
            (index > 0).then(|| index + 1)
        },
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn trace_ordinals_match_bytewise_v1_ids() {
        for len in [0, 1, 9, 10, 11, 99, 100, 101, 1001, 25_001] {
            let mut expected = (0..len).collect::<Vec<_>>();
            expected.sort_by_cached_key(|index| format!("attempt:check:{index}"));
            assert!(super::indices(len).eq(expected), "{len} points");
        }
    }
}
