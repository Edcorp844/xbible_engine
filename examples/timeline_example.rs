use log::debug;

fn main() {
    let engine = xbible_engine::XBibleEngine::new();
    let data = engine.timeline().get_data();
    for period in data {
        debug!("Period : {}", period.title);

        for event in period.events {
            debug!("{} ,", event.title);
        }

        debug!("");
    }
}
