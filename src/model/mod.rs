//! Domain types. Deliberately free of `sea_orm`: everything here is a plain struct
//! that a unit test can build by hand. [`crate::store`] converts rows into these.
//!
//! Every map/set in this module is `BTreeMap`/`BTreeSet`, never `HashMap`/`HashSet` — iteration
//! order here is observable downstream (generated JSON Schema, fan-out slot assignment), and I7
//! requires it to be stable.

mod api_call;
mod auth;
mod budget;
mod endpoint;
mod origin;
mod param;
mod projection;
mod script;
mod service;
mod slug;
mod tag;

pub use api_call::{Access, ApiCall, Pagination};
pub use auth::{AuthKind, AuthProvider, CredentialSource};
pub use budget::Budgets;
pub use endpoint::{EndpointDef, EndpointTarget};
pub use origin::{Origin, OriginError};
pub use param::{
    AllowedHeaderName, HEADER_PARAM_ALLOWLIST, HeaderNameNotAllowed, Param, ParamLocation,
    ParamType, is_header_param_name_allowed,
};
pub use projection::{Cardinality, Projection, ProjectionField};
pub use script::ScriptDef;
pub use service::Service;
pub use slug::{Slug, SlugError};
pub use tag::{Tag, TagExpr, eval as eval_tag_expr};
