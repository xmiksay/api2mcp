//! JSONB <-> typed conversion for `api_calls.projection`/`api_calls.pagination`, split out
//! of `api_call.rs` to keep it under the line cap. Neither [`crate::model::Projection`] nor
//! [`crate::model::Pagination`] derives `serde`, so these small mirror structs exist purely
//! to give `serde_json` something to (de)serialize; converting to/from the real model types
//! happens field-by-field right after.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Cardinality, Pagination, Projection, ProjectionField};

use super::StoreError;
use super::api_call_params::{data_type_to_str, str_to_data_type};

#[derive(Serialize, Deserialize)]
struct ProjectionJson {
    fields: Vec<ProjectionFieldJson>,
}

#[derive(Serialize, Deserialize)]
struct ProjectionFieldJson {
    name: String,
    path: String,
    cardinality: CardinalityJson,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    coerce: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CardinalityJson {
    One,
    Many,
}

pub(super) fn projection_to_json(projection: &Projection) -> Value {
    let json = ProjectionJson {
        fields: projection
            .fields
            .iter()
            .map(|f| ProjectionFieldJson {
                name: f.name.clone(),
                path: f.path.clone(),
                cardinality: match f.cardinality {
                    Cardinality::One => CardinalityJson::One,
                    Cardinality::Many => CardinalityJson::Many,
                },
                coerce: f.coerce.map(data_type_to_str).map(str::to_owned),
            })
            .collect(),
    };
    // Constructed from a valid `ProjectionJson` above; serialization of this shape cannot fail.
    serde_json::to_value(json).expect("ProjectionJson serialization is infallible for this shape")
}

pub(super) fn json_to_projection(value: &Value) -> Result<Projection, StoreError> {
    let parsed: ProjectionJson = serde_json::from_value(value.clone())
        .map_err(|e| StoreError::Malformed(format!("api_calls.projection: {e}")))?;
    let fields = parsed
        .fields
        .into_iter()
        .map(|f| {
            Ok(ProjectionField {
                name: f.name,
                path: f.path,
                cardinality: match f.cardinality {
                    CardinalityJson::One => Cardinality::One,
                    CardinalityJson::Many => Cardinality::Many,
                },
                coerce: f.coerce.as_deref().map(str_to_data_type).transpose()?,
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    Ok(Projection { fields })
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PaginationJson {
    None,
    Cursor {
        next_cursor_path: String,
        query_param: String,
    },
}

pub(super) fn pagination_to_json(pagination: &Pagination) -> Option<Value> {
    match pagination {
        Pagination::None => None,
        Pagination::Cursor {
            next_cursor_path,
            query_param,
        } => Some(
            serde_json::to_value(PaginationJson::Cursor {
                next_cursor_path: next_cursor_path.to_string(),
                query_param: query_param.clone(),
            })
            .expect("PaginationJson serialization is infallible for this shape"),
        ),
    }
}

pub(super) fn json_to_pagination(value: Option<&Value>) -> Result<Pagination, StoreError> {
    let Some(value) = value else {
        return Ok(Pagination::None);
    };
    let parsed: PaginationJson = serde_json::from_value(value.clone())
        .map_err(|e| StoreError::Malformed(format!("api_calls.pagination: {e}")))?;
    match parsed {
        PaginationJson::None => Ok(Pagination::None),
        PaginationJson::Cursor {
            next_cursor_path,
            query_param,
        } => {
            let ptr: jsonptr::PointerBuf = next_cursor_path.parse().map_err(|e| {
                StoreError::Malformed(format!("api_calls.pagination.next_cursor_path: {e}"))
            })?;
            Ok(Pagination::Cursor {
                next_cursor_path: ptr,
                query_param,
            })
        }
    }
}
