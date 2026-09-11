//! Database access. Every function here returns [`crate::model`] types — an
//! `entity::Model` must never escape this module, or the layers above it stop being
//! testable without Postgres.
