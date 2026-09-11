//! `auth_providers` façade. I5 lives here structurally: every write method is
//! `pub(crate)`, so only `pack::import` and `cli` (this crate's two humans-in-the-loop) can
//! bind a credential to an origin — no MCP tool or read-only API route can reach them
//! because they can't even import this module's write surface.
//!
//! The DB's `kind` CHECK allows three values (`header`, `bearer`,
//! `oauth2_client_credentials`) but [`crate::model::AuthKind`] has two variants: `header`
//! and `bearer` both describe a static credential written into one header, which is
//! exactly [`crate::model::AuthKind::StaticHeader`] — the distinction between them is
//! cosmetic once `header_name`/`value_template` fully describe the header. This store
//! folds both into `StaticHeader` on read and always writes `"header"` back, so a
//! provider created as `"bearer"` round-trips as `StaticHeader`, not as its original
//! spelling — a deliberate, documented lossy mapping, not a bug.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use uuid::Uuid;

use crate::entity::auth_providers;
use crate::model::{AuthKind, AuthProvider, Slug};

use super::meta::MetaStore;
use super::{
    ServiceStore, StoreError, db_err, json_string_array, parse_origin, parse_slug, strings_to_json,
};

#[derive(Clone)]
pub struct AuthProviderStore {
    db: DatabaseConnection,
}

impl AuthProviderStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn get(
        &self,
        service_slug: &Slug,
        slug: &Slug,
    ) -> Result<Option<AuthProvider>, StoreError> {
        let service_id = match self.services().id_by_slug(service_slug).await {
            Ok(id) => id,
            Err(StoreError::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let row = auth_providers::Entity::find()
            .filter(auth_providers::Column::ServiceId.eq(service_id))
            .filter(auth_providers::Column::Slug.eq(slug.as_str()))
            .one(&self.db)
            .await
            .map_err(db_err("auth_provider::get"))?;
        row.map(|r| to_model(r, service_slug.clone())).transpose()
    }

    pub async fn list_for_service(
        &self,
        service_slug: &Slug,
    ) -> Result<Vec<AuthProvider>, StoreError> {
        let service_id = self.services().id_by_slug(service_slug).await?;
        let rows = auth_providers::Entity::find()
            .filter(auth_providers::Column::ServiceId.eq(service_id))
            .order_by_asc(auth_providers::Column::Slug)
            .all(&self.db)
            .await
            .map_err(db_err("auth_provider::list_for_service"))?;
        rows.into_iter()
            .map(|r| to_model(r, service_slug.clone()))
            .collect()
    }

    /// I5: a human (via `pack::import` or `cli`) is the only caller that may bind a
    /// credential to an origin. Those callers don't exist yet (later chunks); until then this
    /// is only reachable from this module's own unit tests, hence the `cfg_attr` below —
    /// scoped to non-test builds only, so it can never mask a real regression.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn create(&self, provider: &AuthProvider) -> Result<(), StoreError> {
        let service_id = self.services().id_by_slug(&provider.service_slug).await?;
        if self
            .get(&provider.service_slug, &provider.slug)
            .await?
            .is_some()
        {
            return Err(StoreError::Conflict(format!(
                "auth provider {:?} already exists on service {:?}",
                provider.slug.as_str(),
                provider.service_slug.as_str()
            )));
        }
        let txn = self
            .db
            .begin()
            .await
            .map_err(db_err("auth_provider::create"))?;
        to_active_model(provider, Uuid::new_v4(), service_id)
            .insert(&txn)
            .await
            .map_err(db_err("auth_provider::create"))?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("auth_provider::create"))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn update(&self, provider: &AuthProvider) -> Result<(), StoreError> {
        let service_id = self.services().id_by_slug(&provider.service_slug).await?;
        let id = self
            .id_by_slug(&provider.service_slug, &provider.slug)
            .await?;
        let txn = self
            .db
            .begin()
            .await
            .map_err(db_err("auth_provider::update"))?;
        let mut active = to_active_model(provider, id, service_id);
        active.updated_at = Set(Utc::now().into());
        active
            .update(&txn)
            .await
            .map_err(db_err("auth_provider::update"))?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("auth_provider::update"))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn delete(&self, service_slug: &Slug, slug: &Slug) -> Result<(), StoreError> {
        let id = self.id_by_slug(service_slug, slug).await?;
        let txn = self
            .db
            .begin()
            .await
            .map_err(db_err("auth_provider::delete"))?;
        auth_providers::Entity::delete_by_id(id)
            .exec(&txn)
            .await
            .map_err(db_err("auth_provider::delete"))?;
        MetaStore::bump_generation_in(&txn).await?;
        txn.commit().await.map_err(db_err("auth_provider::delete"))
    }

    pub(crate) async fn id_by_slug(
        &self,
        service_slug: &Slug,
        slug: &Slug,
    ) -> Result<Uuid, StoreError> {
        let service_id = self.services().id_by_slug(service_slug).await?;
        auth_providers::Entity::find()
            .filter(auth_providers::Column::ServiceId.eq(service_id))
            .filter(auth_providers::Column::Slug.eq(slug.as_str()))
            .one(&self.db)
            .await
            .map_err(db_err("auth_provider::id_by_slug"))?
            .map(|r| r.id)
            .ok_or(StoreError::NotFound)
    }

    /// Resolves a bare auth-provider slug to its primary key, without a service to
    /// disambiguate — needed by `store::endpoint`, since `EndpointDef::auth_providers` (a
    /// fixed part of the model) is a `BTreeSet<Slug>`, not a set of `(service, slug)` pairs.
    /// Errors [`StoreError::Conflict`] if more than one service defines a provider with this
    /// slug, since the model has no way to express which one was meant.
    pub(crate) async fn id_by_slug_any_service(&self, slug: &Slug) -> Result<Uuid, StoreError> {
        let mut rows = auth_providers::Entity::find()
            .filter(auth_providers::Column::Slug.eq(slug.as_str()))
            .all(&self.db)
            .await
            .map_err(db_err("auth_provider::id_by_slug_any_service"))?;
        match rows.len() {
            0 => Err(StoreError::NotFound),
            1 => Ok(rows.remove(0).id),
            n => Err(StoreError::Conflict(format!(
                "auth provider slug {:?} is ambiguous: {n} services define it",
                slug.as_str()
            ))),
        }
    }

    /// Resolves an auth provider's `(service_slug, slug)` from its primary key — used by
    /// `store::api_call` to turn `api_calls.auth_provider_id` into the `Option<Slug>`
    /// `model::ApiCall::auth_provider_slug` carries.
    pub(crate) async fn slug_by_id(&self, id: Uuid) -> Result<Slug, StoreError> {
        let row = auth_providers::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(db_err("auth_provider::slug_by_id"))?
            .ok_or(StoreError::NotFound)?;
        parse_slug(&row.slug)
    }

    fn services(&self) -> ServiceStore {
        ServiceStore::new(self.db.clone())
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn to_active_model(
    provider: &AuthProvider,
    id: Uuid,
    service_id: Uuid,
) -> auth_providers::ActiveModel {
    let kind = match provider.kind {
        AuthKind::StaticHeader => "header",
        AuthKind::OAuth2ClientCredentials => "oauth2_client_credentials",
    };
    auth_providers::ActiveModel {
        id: Set(id),
        service_id: Set(service_id),
        slug: Set(provider.slug.as_str().to_owned()),
        kind: Set(kind.to_owned()),
        credential_env_key: Set(provider.credential_env_key.clone()),
        header_name: Set(Some(provider.header_name.clone())),
        value_template: Set(Some(provider.value_template.clone())),
        scopes: Set(Some(strings_to_json(&provider.scopes))),
        token_url: Set(provider.token_url.as_ref().map(|u| u.to_string())),
        bound_origin: Set(provider.bound_origin.to_string()),
        ..Default::default()
    }
}

fn to_model(row: auth_providers::Model, service_slug: Slug) -> Result<AuthProvider, StoreError> {
    let kind = match row.kind.as_str() {
        "header" | "bearer" => AuthKind::StaticHeader,
        "oauth2_client_credentials" => AuthKind::OAuth2ClientCredentials,
        other => {
            return Err(StoreError::Malformed(format!(
                "auth_providers.kind: unrecognised value {other:?}"
            )));
        }
    };
    let header_name = row.header_name.ok_or_else(|| {
        StoreError::Malformed("auth_providers.header_name is required but was NULL".into())
    })?;
    let value_template = row.value_template.ok_or_else(|| {
        StoreError::Malformed("auth_providers.value_template is required but was NULL".into())
    })?;
    let scopes = match row.scopes {
        Some(v) => json_string_array(&v, "auth_providers.scopes")?,
        None => Vec::new(),
    };
    let token_url = row.token_url.as_deref().map(super::parse_url).transpose()?;

    Ok(AuthProvider {
        slug: parse_slug(&row.slug)?,
        service_slug,
        kind,
        credential_env_key: row.credential_env_key,
        header_name,
        value_template,
        scopes,
        token_url,
        bound_origin: parse_origin(&row.bound_origin)?,
    })
}

// I5's write path (`create`/`update`/`delete`) is `pub(crate)`, so an integration test under
// `tests/` — a separate crate — can't reach it at all; these unit tests are the only place
// it gets exercised. See `store::test_support` for why this harness is a small duplicate of
// `tests/common::ScratchDb` rather than a shared dependency.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Origin, Service};
    use crate::store::test_support::ScratchDb;
    use std::collections::{BTreeMap, BTreeSet};

    fn sample_service(slug: &str) -> Service {
        let base_url: url::Url = format!("https://{slug}.example.com/").parse().unwrap();
        Service {
            slug: slug.parse().unwrap(),
            base_url: base_url.clone(),
            origin_allowlist: BTreeSet::from([Origin::of(&base_url).unwrap()]),
            default_headers: BTreeMap::new(),
            timeout_ms: 5_000,
            max_concurrency: 4,
            rate_limit_per_min: None,
            max_response_bytes: 1_000_000,
        }
    }

    fn sample_provider(service_slug: &Slug, slug: &str) -> AuthProvider {
        AuthProvider {
            slug: slug.parse().unwrap(),
            service_slug: service_slug.clone(),
            kind: AuthKind::StaticHeader,
            credential_env_key: "A2M_CRED_TEST".into(),
            header_name: "Authorization".into(),
            value_template: "Bearer {token}".into(),
            scopes: vec!["read".into()],
            token_url: None,
            bound_origin: "https://svc.example.com".parse().unwrap(),
        }
    }

    #[tokio::test]
    async fn create_get_update_delete_roundtrip() {
        let Some(scratch) = ScratchDb::create().await.expect("scratch db") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };

        let services = crate::store::ServiceStore::new(scratch.db.clone());
        let service = sample_service("auth-provider-test-svc");
        services.create(&service).await.expect("create service");

        let store = AuthProviderStore::new(scratch.db.clone());
        let provider = sample_provider(&service.slug, "primary");
        store.create(&provider).await.expect("create provider");

        let fetched = store
            .get(&service.slug, &provider.slug)
            .await
            .expect("get provider")
            .expect("provider exists");
        assert_eq!(fetched, provider);

        let mut updated = provider.clone();
        updated.header_name = "X-Api-Key".into();
        updated.value_template = "{token}".into();
        store.update(&updated).await.expect("update provider");
        let refetched = store
            .get(&service.slug, &provider.slug)
            .await
            .expect("get provider")
            .expect("provider still exists");
        assert_eq!(refetched, updated);

        store
            .delete(&service.slug, &provider.slug)
            .await
            .expect("delete provider");
        assert!(
            store
                .get(&service.slug, &provider.slug)
                .await
                .expect("get after delete")
                .is_none()
        );

        scratch.teardown().await.expect("teardown");
    }
}
