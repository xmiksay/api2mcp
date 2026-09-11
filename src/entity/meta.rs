//! `meta` — a generic key/value store. Its one load-bearing row today is
//! `definitions_generation`, the counter [`crate::resolve`]'s plan cache is keyed on: bump
//! it on any definition write and every cached `EndpointPlan` invalidates.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "meta")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub key: String,
    pub value: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
