#[macro_use]
extern crate rocket;
use rocket_lamb::RocketExt;

#[get("/")]
fn hello() -> &'static str {
    "Hello, world!"
}

#[tokio::main]
async fn main() {
    rocket::ignite()
        .mount("/hello", routes![hello])
        .lambda()
        .launch().await;
}
