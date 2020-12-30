#[cfg(target_pointer_width = "64")]
mod x86_64;

#[cfg(target_pointer_width = "64")]
pub mod current {
	pub use super::x86_64::*;
}