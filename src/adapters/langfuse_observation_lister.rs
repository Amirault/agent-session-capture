//! What the Langfuse reader needs from the API.
//!
//! An observation is the raw JSON object of the v2 endpoint
//! (`GET /api/public/v2/observations`, field groups
//! `core,basic,io,metadata,trace_context`, or `core,basic,time` for the
//! activity pass). The fields used: `id`, `traceId`, `sessionId`, `type`,
//! `name`, `startTime`, `endTime`, `parentObservationId`,
//! `isRootObservation`, `input`, `output` (raw strings), `metadata`,
//! `updatedAt`.

use crate::domain::source_errors::ReadError;
use crate::js::Object;

/// A Langfuse v2 observation.
pub type LangfuseObservation = Object;

/// Filters of one listing (each optional).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObservationQuery {
    /// ISO-8601: only observations started at/after it (none: the whole project).
    pub from_start_time: Option<String>,
    /// Only the observations carrying this session: the root of each of its traces.
    pub session_id: Option<String>,
    /// Only the observations of this trace.
    pub trace_id: Option<String>,
}

/// Every observation matching the query, all pages.
pub trait LangfuseObservationLister {
    /// Observations with their content (input, output, metadata).
    fn list_observations(&self, query: &ObservationQuery) -> Result<Vec<LangfuseObservation>, ReadError>;
    /// Observations without content: ids, `sessionId` and `updatedAt` only.
    fn list_activity(&self, query: &ObservationQuery) -> Result<Vec<LangfuseObservation>, ReadError>;
}
