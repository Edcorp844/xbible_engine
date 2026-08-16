use xbible_engine::data::timeline_data::data::TimelineData;

fn main(){
    xbible_engine::init_logging();
    let data = TimelineData::new().get_data();
     for period in data {
        println!("Period : {}", period.title);

        for event in period.events {
            print!("{} ,", event.title);
        }

        println!("");
     }
}