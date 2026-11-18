use log::debug;

fn main() {
    xbible_engine::init_logging();
    let data = TimelineData::new().get_data();
    for period in data {
        debug!("Period : {}", period.title);

        for event in period.events {
            debug!("{} ,", event.title);
        }

        debug!("");
    }
}
