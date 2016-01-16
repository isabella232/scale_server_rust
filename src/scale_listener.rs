use std::io;

use std::collections::HashMap;
use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread;

type ObserverId = usize;

pub struct ScaleListener {
    next_observer_id: ObserverId,
    observers: HashMap<ObserverId, Sender<String>>,
}

impl ScaleListener {
    fn new() -> ScaleListener {
        ScaleListener {
            next_observer_id: 0,
            observers: HashMap::new(),
        }
    }

    pub fn listen<A: ToSocketAddrs>(listen_addr: A) -> io::Result<Arc<Mutex<ScaleListener>>> {
        let listener = Arc::new(Mutex::new(ScaleListener::new()));

        let thread_listener = listener.clone();
        let socket = try!(UdpSocket::bind(listen_addr));

        thread::spawn(move || {
            let mut buf = [0; 1024];

            loop {
                let (amt, _) = match socket.recv_from(&mut buf) {
                    Ok(val) => val,
                    Err(err) => {
                        error!("error reading upd scale packet: {}", err);
                        continue;
                    }
                };

                match String::from_utf8(Vec::from(&buf[0..amt])) {
                    Ok(msg) => thread_listener.lock().unwrap().notify_observers(msg),
                    Err(err) => {
                        error!("error decoding scale message: {}", err);
                    }
                }
            }
        });

        Ok(listener)
    }

    pub fn add_observer(&mut self) -> (ObserverId, Receiver<String>) {
        let (tx, rx) = channel();
        let id = self.next_observer_id;

        self.observers.insert(id, tx);
        self.next_observer_id += 1;

        (id, rx)
    }

    pub fn remove_observer(&mut self, id: &ObserverId) {
        self.observers.remove(id);
    }

    fn notify_observers(&mut self, msg: String) {
        let mut remove_ids = Vec::new();

        for (id, channel) in self.observers.iter() {
            if channel.send(msg.clone()).is_err() {
                remove_ids.push(*id);
            }
        }

        for id in remove_ids {
            self.observers.remove(&id);
        }
    }
}
