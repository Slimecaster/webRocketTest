#[macro_use] extern crate rocket;

use rocket::{State, Shutdown};
use rocket::fs::{relative, FileServer};
use rocket::form::Form;
use rocket::response::stream::{EventStream, Event};
use rocket::serde::{Serialize, Deserialize};
use rocket::tokio::{self, sync::broadcast::{channel, Sender, error::RecvError}};
use rocket::tokio::select;
use serial2::SerialPort;
use std::thread;
use std::time::Duration;
use std::sync::{Arc, Mutex};

/// Message structure (now supports Micro:bit messages)
#[derive(Debug, Clone, FromForm, Serialize, Deserialize)]
#[serde(crate = "rocket::serde")]
struct Message {
    #[field(validate = len(..30))]
    pub room: String,
    #[field(validate = len(..20))]
    pub username: String,
    pub message: String,
}

/// Stream events to clients
#[get("/events")]
async fn events(queue: &State<Sender<Message>>, mut end: Shutdown) -> EventStream![] {
    let mut rx = queue.subscribe();
    EventStream! {
        loop {
            let msg = select! {
                msg = rx.recv() => match msg {
                    Ok(msg) => msg,
                    Err(RecvError::Closed) => break,
                    Err(RecvError::Lagged(_)) => continue,
                },
                _ = &mut end => break,
            };

            yield Event::json(&msg);
        }
    }
}

/// Receive a chat message
#[post("/message", data = "<form>")]
fn post(form: Form<Message>, queue: &State<Sender<Message>>, port: &State<Arc<Mutex<SerialPort>>>) {
    let msg = form.into_inner();
    let _res = queue.send(msg.clone());

    // Send the message to the Micro:bit
    if let Ok(mut port) = port.lock() {
        let _ = port.write_all(msg.message.as_bytes());
        let _ = port.write_all(b"\n"); // Ensure newline for Micro:bit parsing
    }
}

/// Background thread to read from Micro:bit
fn start_microbit_reader(queue: Sender<Message>, port: Arc<Mutex<SerialPort>>) {
    thread::spawn(move || {
        let mut buffer = [0; 64];

        loop {
            if let Ok(mut port) = port.lock() {
                if let Ok(n) = port.read(&mut buffer) {
                    let received = String::from_utf8_lossy(&buffer[..n]).trim().to_string();
                    if !received.is_empty() {
                        let msg = Message {
                            room: "Micro:bit".to_string(),
                            username: "Micro:bit".to_string(),
                            message: received,
                        };
                        let _ = queue.send(msg);
                    }
                }
            }
            thread::sleep(Duration::from_millis(500)); // Avoid excessive polling
        }
    });
}

#[launch]
fn rocket() -> _ {
    let (tx, _) = channel::<Message>(1024);

    // Open the serial port for both reading and writing
    let serial_port = Arc::new(Mutex::new(
        SerialPort::open("COM4", 115200).expect("Failed to open serial port")
    ));

    start_microbit_reader(tx.clone(), serial_port.clone()); // Start the Micro:bit reader

    rocket::build()
        .manage(tx)
        .manage(serial_port)
        .mount("/", routes![post, events])
        .mount("/", FileServer::from(relative!("static")))
}
