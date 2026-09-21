//! The bindings generator, built from this crate so it always matches the
//! `uniffi` version the library was compiled against.
//!
//! A mismatch between generator and runtime produces bindings that compile and
//! then misbehave at the boundary, which is the worst way to find out.

fn main() {
    uniffi::uniffi_bindgen_main()
}
