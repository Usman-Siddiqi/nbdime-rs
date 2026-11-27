// A plain `cargo build` of the extension module needs `-undefined dynamic_lookup` on macOS;
// maturin passes it itself, this covers builds outside maturin.
fn main() {
    pyo3_build_config::add_extension_module_link_args();
}
