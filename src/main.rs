mod collector;
mod database;

fn main() {
    database::init_database();
    collector::build_collector();
}