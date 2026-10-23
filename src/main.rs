use anyhow::{Context, Result};
use rsnotebook::config::{self, Config};
use rsnotebook::store::Store;

fn main() -> Result<()> {
    install_panic_hook();
    let cfg = Config::load(None);
    let db_path = config::db_path();
    let store = Store::open(&db_path)
        .with_context(|| format!("could not open database {}", db_path.display()))?;
    rsnotebook::app::run(store, cfg)
}

/// Panics on the main thread restore the terminal before printing. Panics on other
/// threads (a PDF parser choking on a malformed file, say) are caught and reported
/// by the code that spawned them, so they must not draw over the screen.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            rsnotebook::app::restore_terminal();
            default(info);
        }
    }));
}
