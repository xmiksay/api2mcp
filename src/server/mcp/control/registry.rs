//! `tools/list`'s full payload for the control plane: every resource module's own
//! [`descriptors`]-named function, concatenated. Unlike [`super::super::registry`] (the data
//! plane's tool renderer), there is no per-request plan here to render — the control plane's
//! tool set is fixed, so this list is the same for every call.

use serde_json::Value;

use super::{api_calls, endpoints, runs_tags, scripts, services};

pub(super) fn descriptors() -> Vec<Value> {
    let mut tools = Vec::new();
    tools.extend(services::descriptors());
    tools.extend(api_calls::descriptors());
    tools.extend(scripts::descriptors());
    tools.extend(endpoints::descriptors());
    tools.extend(runs_tags::descriptors());
    tools
}
