//! A test client: `call IFACE METHOD [s:TEXT | b:true | x:42]...` prints the reply's
//! values, or `error: NAME: MESSAGE`. tests/dbus.sh copies it next to the daemon under
//! the names of the programs the daemon trusts, to check what each may do.

use nyx_clipboard_dbus::{ACTIONS, BUS, CLIPBOARD, CODES, PATH};
use zbus::zvariant::{OwnedValue, StructureBuilder, Value};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(iface), Some(method)) = (args.first(), args.get(1)) else {
        eprintln!("usage: call Clipboard1|Actions1|Otp1 METHOD [s:… b:… x:…]");
        std::process::exit(2);
    };
    let iface = match iface.as_str() {
        "Clipboard1" => CLIPBOARD,
        "Actions1" => ACTIONS,
        "Otp1" => CODES,
        other => other,
    };
    let mut body = StructureBuilder::new();
    for a in &args[2..] {
        let v: Value = match a.split_once(':') {
            Some(("s", t)) => Value::from(t.to_string()),
            Some(("b", t)) => Value::from(t == "true"),
            Some(("x", t)) => Value::from(t.parse::<i64>().unwrap_or(0)),
            _ => Value::from(a.clone()),
        };
        body = body.append_field(v);
    }
    let conn = zbus::blocking::Connection::session().expect("session bus");
    let reply = if args.len() > 2 {
        conn.call_method(Some(BUS), PATH, Some(iface), method.as_str(), &body.build().expect("body"))
    } else {
        conn.call_method(Some(BUS), PATH, Some(iface), method.as_str(), &())
    };
    match reply {
        Ok(m) => {
            let b = m.body();
            match b.signature().to_string().as_str() {
                "b" => println!("{}", b.deserialize::<bool>().unwrap_or(false)),
                "s" => println!("{}", b.deserialize::<String>().unwrap_or_default()),
                "u" => println!("{}", b.deserialize::<u32>().unwrap_or(0)),
                "x" => println!("{}", b.deserialize::<i64>().unwrap_or(0)),
                "" => println!("ok"),
                _ => match b.deserialize::<OwnedValue>() {
                    Ok(v) => println!("{v:?}"),
                    Err(_) => println!("ok"),
                },
            }
        }
        Err(zbus::Error::MethodError(name, msg, _)) => {
            println!("error: {}: {}", name, msg.unwrap_or_default());
            std::process::exit(1);
        }
        Err(e) => {
            println!("error: {e}");
            std::process::exit(1);
        }
    }
}
