
pub mod error;
pub mod manager;
mod verify;
pub mod lights;
mod scope;
mod values;
mod effects;
#[cfg(test)]
mod tests;

pub use error::DmxArrayError;
pub use manager::ArrayManager;
pub(super) use scope::Scope;
