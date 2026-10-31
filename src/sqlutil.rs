//! 动态 SQL 参数：条件数量随查询参数变化时按顺序绑定。

use sqlx::{
    query::{QueryAs, QueryScalar},
    sqlite::SqliteArguments,
    Sqlite,
};

pub enum Param {
    Text(String),
    Int(i64),
}

/// `IN (...)` 的占位符串，例如 3 -> "?,?,?"。
pub fn placeholders(count: usize) -> String {
    vec!["?"; count].join(",")
}

/// 可追加绑定的语句类型（`query_as` / `query_scalar` 返回的不是同一个类型）。
pub trait Bindable<'q>: Sized {
    fn bind_param(self, param: &Param) -> Self;
}

impl<'q, O> Bindable<'q> for QueryAs<'q, Sqlite, O, SqliteArguments<'q>> {
    fn bind_param(self, param: &Param) -> Self {
        match param {
            Param::Text(value) => self.bind(value.clone()),
            Param::Int(value) => self.bind(*value),
        }
    }
}

impl<'q, O> Bindable<'q> for QueryScalar<'q, Sqlite, O, SqliteArguments<'q>> {
    fn bind_param(self, param: &Param) -> Self {
        match param {
            Param::Text(value) => self.bind(value.clone()),
            Param::Int(value) => self.bind(*value),
        }
    }
}

pub fn bind_all<'q, Q: Bindable<'q>>(mut query: Q, params: &[Param]) -> Q {
    for param in params {
        query = query.bind_param(param);
    }
    query
}
