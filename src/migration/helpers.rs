//! Column builders shared by the migrations. Every table wants the same UUID
//! primary key and `TIMESTAMPTZ NOT NULL DEFAULT now()` pair; hand-rolling them in
//! each `mNNNN` file would repeat the same lines across all seven migrations.

use sea_orm_migration::prelude::*;

/// `<name> UUID NOT NULL PRIMARY KEY DEFAULT gen_random_uuid()`.
pub fn uuid_pk(name: impl IntoIden) -> ColumnDef {
    let mut c = ColumnDef::new(name);
    c.uuid()
        .not_null()
        .primary_key()
        .default(Expr::cust("gen_random_uuid()"));
    c
}

/// A not-null UUID column with no default — the usual foreign-key-column shape.
pub fn uuid_col(name: impl IntoIden) -> ColumnDef {
    let mut c = ColumnDef::new(name);
    c.uuid().not_null();
    c
}

/// `<name> TIMESTAMPTZ NOT NULL DEFAULT now()`.
pub fn timestamptz_now(name: impl IntoIden) -> ColumnDef {
    let mut c = ColumnDef::new(name);
    c.timestamp_with_time_zone()
        .not_null()
        .default(Expr::current_timestamp());
    c
}

/// A nullable `TIMESTAMPTZ`, no default — deadlines and "last used" markers.
pub fn timestamptz_null(name: impl IntoIden) -> ColumnDef {
    let mut c = ColumnDef::new(name);
    c.timestamp_with_time_zone().null();
    c
}
