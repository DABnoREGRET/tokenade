#[macro_use]
extern crate rocket;

#[get("/")]
fn index() -> &'static str { "hello" }

#[post("/login")]
fn login() -> &'static str { "ok" }

#[launch]
fn rocket() -> _ {
    rocket::build().mount("/", routes![index, login])
}
