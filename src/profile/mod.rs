//! Provider profiles: built-in tier presets (template) + user-defined named profiles (named).

// Re-export everything from the existing template module for backward compatibility.
mod template;
pub use template::*;

// Named runtime profiles (the new feature).
pub mod named;
