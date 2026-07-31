pub mod fsops;
pub mod fsview;
pub mod osc7;
pub mod pty;
pub mod sync;
pub mod terminal;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
