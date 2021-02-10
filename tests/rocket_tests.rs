#![feature(proc_macro_hygiene, decl_macro)]

#[macro_use]
extern crate rocket;

use lamedh_http::{Body, Handler, Request, Response};
use lamedh_runtime::Context;
use rocket_lamb::{ResponseType, RocketExt};
use std::error::Error;
use std::fs::File;

#[catch(404)]
fn not_found() {}

#[post("/upper/<path>?<query>", data = "<body>")]
fn upper(path: String, query: String, body: String) -> String {
    format!(
        "{}, {}, {}",
        path.to_uppercase(),
        query.to_uppercase(),
        body.to_uppercase()
    )
}

#[get("/binary")]
fn binary() -> &'static [u8] {
    &[200, 201, 202]
}

fn make_rocket() -> rocket::Rocket {
    rocket::ignite()
        .mount("/", routes![upper, binary])
        .register(catchers![not_found])
}

fn get_request(json_file: &'static str) -> Result<Request, Box<dyn Error>> {
    let file = File::open(format!("tests/requests/{}.json", json_file))?;
    Ok(lamedh_http::request::from_reader(file)?)
}

#[tokio::test]
async fn ok_auto_text() -> Result<(), Box<dyn Error>> {
    let mut handler = make_rocket().lambda().into_handler();

    let req = get_request("upper")?;
    let res = handler.call(req, Context::default()).await?;

    assert_eq!(res.status(), 200);
    assert_header(&res, "content-type", "text/plain; charset=utf-8");
    assert_eq!(*res.body(), Body::Text("ONE, TWO, THREE".to_string()));
    Ok(())
}

#[tokio::test]
async fn ok_auto_binary() -> Result<(), Box<dyn Error>> {
    let mut handler = make_rocket().lambda().into_handler();

    let req = get_request("binary")?;
    let res = handler.call(req, Context::default()).await?;

    assert_eq!(res.status(), 200);
    assert_header(&res, "content-type", "application/octet-stream");
    assert_eq!(*res.body(), Body::Binary(vec![200, 201, 202]));
    Ok(())
}

#[tokio::test]
async fn ok_default_binary() -> Result<(), Box<dyn Error>> {
    let mut handler = make_rocket()
        .lambda()
        .default_response_type(ResponseType::Binary)
        .into_handler();

    let req = get_request("upper")?;
    let res = handler.call(req, Context::default()).await?;

    assert_eq!(res.status(), 200);
    assert_header(&res, "content-type", "text/plain; charset=utf-8");
    assert_eq!(
        *res.body(),
        Body::Binary("ONE, TWO, THREE".to_owned().into_bytes())
    );
    Ok(())
}

#[tokio::test]
async fn ok_type_binary() -> Result<(), Box<dyn Error>> {
    let mut handler = make_rocket()
        .lambda()
        .response_type("TEXT/PLAIN", ResponseType::Binary)
        .into_handler();

    let req = get_request("upper")?;
    let res = handler.call(req, Context::default()).await?;

    assert_eq!(res.status(), 200);
    assert_header(&res, "content-type", "text/plain; charset=utf-8");
    assert_eq!(
        *res.body(),
        Body::Binary("ONE, TWO, THREE".to_owned().into_bytes())
    );
    Ok(())
}

#[tokio::test]
async fn request_not_found() -> Result<(), Box<dyn Error>> {
    let mut handler = make_rocket().lambda().into_handler();

    let req = get_request("not_found")?;
    let res = handler.call(req, Context::default()).await?;

    assert_eq!(res.status(), 404);
    assert_eq!(res.headers().contains_key("content-type"), false);
    assert!(res.body().is_empty(), "Response body should be empty");
    Ok(())
}

fn assert_header(res: &Response<Body>, name: &str, value: &str) {
    let values = res.headers().get_all(name).iter().collect::<Vec<_>>();
    assert_eq!(values.len(), 1, "Header {} should have 1 value", name);
    assert_eq!(values[0], value);
}
