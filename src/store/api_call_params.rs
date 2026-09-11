//! `api_call_params` read/write, split out of `api_call.rs` to keep both files under the
//! line cap. `Param::location` encodes what `api_call_params` splits across two columns
//! (`location`, `body_path`); `ParamLocation::Local` has no representation here at all — a
//! script-only concept the `script_params` table (no `location` column) exists precisely
//! because it doesn't need — so writing one is a caller bug, not a malformed row, and
//! reported as [`StoreError::Conflict`].

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use crate::entity::api_call_params;
use crate::model::{Param, ParamLocation, ParamType};

use super::{StoreError, db_err, enum_values_to_json, json_to_enum_values};

/// Deletes every existing param row for `api_call_id` and inserts `params` in their given
/// order. Runs over a generic connection so the caller can include it in its own
/// transaction alongside the parent `api_calls` row write.
pub(crate) async fn replace_params<C: ConnectionTrait>(
    conn: &C,
    api_call_id: Uuid,
    params: &[Param],
) -> Result<(), StoreError> {
    api_call_params::Entity::delete_many()
        .filter(api_call_params::Column::ApiCallId.eq(api_call_id))
        .exec(conn)
        .await
        .map_err(db_err("api_call_params::replace_params"))?;
    for p in params {
        to_active_model(p, api_call_id, Uuid::new_v4())?
            .insert(conn)
            .await
            .map_err(db_err("api_call_params::replace_params"))?;
    }
    Ok(())
}

/// Loads every param for `api_call_id`, ordered by `position` (I7) — the order
/// `schema::input_schema` and fan-out both require.
pub(crate) async fn load_params<C: ConnectionTrait>(
    conn: &C,
    api_call_id: Uuid,
) -> Result<Vec<Param>, StoreError> {
    let rows = api_call_params::Entity::find()
        .filter(api_call_params::Column::ApiCallId.eq(api_call_id))
        .order_by_asc(api_call_params::Column::Position)
        .all(conn)
        .await
        .map_err(db_err("api_call_params::load_params"))?;
    rows.into_iter().map(to_model).collect()
}

fn to_active_model(
    p: &Param,
    api_call_id: Uuid,
    id: Uuid,
) -> Result<api_call_params::ActiveModel, StoreError> {
    let (location, body_path) = match &p.location {
        ParamLocation::Path => ("path", None),
        ParamLocation::Query => ("query", None),
        ParamLocation::Header => ("header", None),
        ParamLocation::Body(ptr) => ("body", Some(ptr.to_string())),
        ParamLocation::Local => {
            return Err(StoreError::Conflict(format!(
                "param {:?}: Local params cannot be stored in api_call_params (script params only)",
                p.name
            )));
        }
    };
    Ok(api_call_params::ActiveModel {
        id: Set(id),
        api_call_id: Set(api_call_id),
        name: Set(p.name.clone()),
        location: Set(location.to_owned()),
        data_type: Set(data_type_to_str(p.ty).to_owned()),
        required: Set(p.required),
        default_value: Set(p.default.clone()),
        fixed_value: Set(p.fixed.clone()),
        enum_values: Set(enum_values_to_json(&p.enum_values)),
        body_path: Set(body_path),
        position: Set(p.position),
        description: Set(p.description.clone().unwrap_or_default()),
    })
}

fn to_model(row: api_call_params::Model) -> Result<Param, StoreError> {
    let location = match row.location.as_str() {
        "path" => ParamLocation::Path,
        "query" => ParamLocation::Query,
        "header" => ParamLocation::Header,
        "body" => {
            let raw = row.body_path.as_deref().ok_or_else(|| {
                StoreError::Malformed(
                    "api_call_params: location='body' but body_path is NULL".into(),
                )
            })?;
            let ptr: jsonptr::PointerBuf = raw.parse().map_err(|e| {
                StoreError::Malformed(format!("api_call_params.body_path {raw:?}: {e}"))
            })?;
            ParamLocation::Body(ptr)
        }
        other => {
            return Err(StoreError::Malformed(format!(
                "api_call_params.location: unrecognised value {other:?}"
            )));
        }
    };
    Ok(Param {
        name: row.name,
        location,
        ty: str_to_data_type(&row.data_type)?,
        required: row.required,
        default: row.default_value,
        fixed: row.fixed_value,
        enum_values: json_to_enum_values(row.enum_values, "api_call_params.enum_values")?,
        // `''` and "no description" are the same thing to a schema consumer (see the
        // column's migration comment) — collapse the empty-string default back to `None`
        // here rather than exposing a third state to `model::Param`.
        description: (!row.description.is_empty()).then_some(row.description),
        position: row.position,
    })
}

pub(crate) fn data_type_to_str(ty: ParamType) -> &'static str {
    match ty {
        ParamType::String => "string",
        ParamType::Integer => "integer",
        ParamType::Number => "number",
        ParamType::Boolean => "boolean",
        ParamType::StringArray => "array",
    }
}

pub(crate) fn str_to_data_type(s: &str) -> Result<ParamType, StoreError> {
    match s {
        "string" => Ok(ParamType::String),
        "integer" => Ok(ParamType::Integer),
        "number" => Ok(ParamType::Number),
        "boolean" => Ok(ParamType::Boolean),
        "array" => Ok(ParamType::StringArray),
        other => Err(StoreError::Malformed(format!(
            "api_call_params.data_type: unsupported value {other:?}"
        ))),
    }
}
