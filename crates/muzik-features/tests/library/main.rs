#![expect(
    clippy::needless_pass_by_ref_mut,
    clippy::needless_pass_by_value,
    reason = "cucumber step functions take `&mut World` and owned captures"
)]

mod steps;
#[path = "../support.rs"]
mod support;
mod world;

fn main() {
    support::run::<world::LibraryWorld>("library");
}
