//! JSON-schema source. Variant under test is chosen by cargo feature.
use serde_json::Value;
#[cfg(feature = "schemars")]
pub trait HasSchema: schemars::JsonSchema {}
#[cfg(feature = "schemars")]
impl<T: schemars::JsonSchema> HasSchema for T {}
#[cfg(feature = "schemars")]
pub fn schema_of<T: HasSchema>() -> Value { schemars::schema_for!(T).to_value() }

#[cfg(not(feature = "schemars"))]
pub trait HasSchema { fn schema() -> Value; }
#[cfg(not(feature = "schemars"))]
pub fn schema_of<T: HasSchema>() -> Value { T::schema() }
