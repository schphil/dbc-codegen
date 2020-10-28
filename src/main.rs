use anyhow::{anyhow, ensure, Context, Result};
use can_dbc::{Message, MessageId, Signal, DBC};
use heck::{CamelCase, SnakeCase};
use pad::PadAdapter;
use std::{fs::File, io::BufWriter, io::Write, path::PathBuf};
use structopt::StructOpt;

mod includes;
mod pad;

#[derive(Debug, StructOpt)]
struct Cli {
    dbc_path: PathBuf,
    out_path: PathBuf,
}

fn main() -> Result<()> {
    let args = Cli::from_args();
    let dbc = std::fs::read(&args.dbc_path)
        .with_context(|| format!("could not read `{}`", args.dbc_path.display()))?;
    let dbc = can_dbc::DBC::from_slice(&dbc)
        .map_err(|e| anyhow!("Could not parse dbc file: {:#?}", e))?;

    ensure!(
        args.out_path.is_dir(),
        "Output path needs to point to a directory"
    );

    let messages_path = args.out_path.join("messages.rs");
    let messages_code =
        File::create(messages_path).context("Could not create `messages.rs` file")?;
    let mut w = BufWriter::new(messages_code);

    writeln!(&mut w, "// Generated code!")?;
    writeln!(&mut w, "#![no_std]")?;
    writeln!(
        &mut w,
        "#![allow(unused, clippy::let_and_return, clippy::eq_op)]"
    )?;
    writeln!(&mut w)?;
    let dbc_file_name = args
        .dbc_path
        .file_name()
        .unwrap_or_else(|| args.dbc_path.as_ref());
    writeln!(
        &mut w,
        "//! Message definitions from file `{:?}`",
        dbc_file_name
    )?;
    writeln!(&mut w, "//!")?;
    writeln!(&mut w, "//! - Version: `{:?}`", dbc.version())?;
    writeln!(&mut w)?;
    writeln!(&mut w, "use bitsh::Pack;")?;
    writeln!(&mut w)?;

    render_dbc(&dbc, &mut w).context("could not generate Rust code")?;

    writeln!(&mut w)?;
    writeln!(&mut w, "/// This is just to make testing easier")?;
    writeln!(&mut w, "fn main() {{}}")?;
    writeln!(&mut w)?;
    w.write_all(include_bytes!("./includes/errors.rs"))?;
    writeln!(&mut w)?;

    Ok(())
}

fn render_dbc(dbc: &DBC, mut w: impl Write) -> Result<()> {
    render_root_enum(dbc, &mut w)?;

    for msg in dbc.messages() {
        render_message(msg, &mut w, dbc)
            .with_context(|| format!("write message `{}`", msg.message_name()))?;
        writeln!(w)?;
    }

    Ok(())
}

fn render_root_enum(dbc: &DBC, mut w: impl Write) -> Result<()> {
    writeln!(w, "/// All messages")?;
    writeln!(w, "#[derive(Clone)]")?;
    writeln!(w, r##"#[cfg_attr(feature = "debug", derive(Debug))]"##)?;
    writeln!(w, "pub enum Messages {{")?;
    {
        let mut w = PadAdapter::wrap(&mut w);
        for msg in dbc.messages() {
            writeln!(w, "/// {}", msg.message_name())?;
            writeln!(w, "{0}({0}),", type_name(msg.message_name()))?;
        }
    }
    writeln!(&mut w, "}}")?;
    writeln!(&mut w)?;

    writeln!(w, "impl Messages {{")?;
    {
        let mut w = PadAdapter::wrap(&mut w);
        writeln!(&mut w, "/// Read message from CAN frame")?;
        writeln!(w, "#[inline(never)]")?;
        writeln!(
            &mut w,
            "pub fn from_can_message(id: u32, payload: &[u8]) -> Result<Self, CanError> {{",
        )?;
        {
            let mut w = PadAdapter::wrap(&mut w);
            writeln!(&mut w, "use core::convert::TryFrom;")?;
            writeln!(&mut w)?;
            writeln!(&mut w, "let res = match id {{")?;
            {
                let mut w = PadAdapter::wrap(&mut w);
                for msg in dbc.messages() {
                    writeln!(
                        w,
                        "{} => Messages::{1}({1}::try_from(payload)?),",
                        msg.message_id().0,
                        type_name(msg.message_name())
                    )?;
                }
                writeln!(w, r#"n => return Err(CanError::UnknownMessageId(n)),"#)?;
            }
            writeln!(&mut w, "}};")?;
            writeln!(&mut w, "Ok(res)")?;
        }

        writeln!(&mut w, "}}")?;
    }
    writeln!(&mut w, "}}")?;
    writeln!(&mut w)?;

    Ok(())
}

fn render_message(msg: &Message, mut w: impl Write, dbc: &DBC) -> Result<()> {
    writeln!(w, "/// {}", msg.message_name())?;
    writeln!(w, "///")?;
    writeln!(w, "/// - ID: {0} (0x{0:x})", msg.message_id().0)?;
    writeln!(w, "/// - Size: {} bytes", msg.message_size())?;
    if let can_dbc::Transmitter::NodeName(transmitter) = msg.transmitter() {
        writeln!(w, "/// - Transmitter: {}", transmitter)?;
    }
    if let Some(comment) = dbc.message_comment(*msg.message_id()) {
        writeln!(w, "///")?;
        for line in comment.trim().lines() {
            writeln!(w, "/// {}", line)?;
        }
    }
    writeln!(w, "#[derive(Clone, Copy)]")?;
    writeln!(w, r##"#[cfg_attr(feature = "debug", derive(Debug))]"##)?;
    writeln!(w, "pub struct {} {{", type_name(msg.message_name()))?;
    {
        let mut w = PadAdapter::wrap(&mut w);
        writeln!(w, "raw: [u8; {}],", msg.message_size())?;
    }
    writeln!(w, "}}")?;
    writeln!(w)?;

    writeln!(w, "impl {} {{", type_name(msg.message_name()))?;
    {
        let mut w = PadAdapter::wrap(&mut w);

        writeln!(
            &mut w,
            "pub const MESSAGE_ID: u32 = {};",
            msg.message_id().0
        )?;
        writeln!(w)?;

        writeln!(
            &mut w,
            "/// Construct new {} from values",
            msg.message_name()
        )?;
        let args: Vec<String> = msg
            .signals()
            .iter()
            .map(|signal| {
                format!(
                    "{}: {}",
                    field_name(signal.name()),
                    signal_to_rust_type(&signal)
                )
            })
            .collect();
        writeln!(
            &mut w,
            "pub fn new({}) -> Result<Self, CanError> {{",
            args.join(", ")
        )?;
        {
            let mut w = PadAdapter::wrap(&mut w);
            writeln!(
                &mut w,
                "let mut res = Self {{ raw: [0u8; {}] }};",
                msg.message_size()
            )?;
            for signal in msg.signals().iter() {
                writeln!(&mut w, "res.set_{0}({0})?;", field_name(signal.name()))?;
            }
            writeln!(&mut w, "Ok(res)")?;
        }
        writeln!(&mut w, "}}")?;
        writeln!(w)?;

        for signal in msg.signals().iter() {
            render_signal(signal, &mut w, dbc, msg)
                .with_context(|| format!("write signal impl `{}`", signal.name()))?;
        }
    }
    writeln!(w, "}}")?;
    writeln!(w)?;

    writeln!(
        w,
        "impl core::convert::TryFrom<&[u8]> for {} {{",
        type_name(msg.message_name())
    )?;
    {
        let mut w = PadAdapter::wrap(&mut w);
        writeln!(&mut w, "type Error = CanError;")?;
        writeln!(w)?;
        writeln!(w, "#[inline(always)]")?;
        writeln!(
            &mut w,
            "fn try_from(payload: &[u8]) -> Result<Self, Self::Error> {{"
        )?;
        {
            let mut w = PadAdapter::wrap(&mut w);
            writeln!(
                &mut w,
                r#"if payload.len() != {} {{ return Err(CanError::InvalidPayloadSize); }}"#,
                msg.message_size()
            )?;
            writeln!(&mut w, "let mut raw = [0u8; {}];", msg.message_size())?;
            writeln!(
                &mut w,
                "raw.copy_from_slice(&payload[..{}]);",
                msg.message_size()
            )?;
            writeln!(&mut w, "Ok(Self {{ raw }})")?;
        }
        writeln!(&mut w, "}}")?;
    }

    writeln!(w, "}}")?;
    writeln!(w)?;

    Ok(())
}

fn render_signal(signal: &Signal, mut w: impl Write, dbc: &DBC, msg: &Message) -> Result<()> {
    writeln!(w, "/// {}", signal.name())?;
    if let Some(comment) = dbc.signal_comment(*msg.message_id(), &signal.name()) {
        writeln!(w, "///")?;
        for line in comment.trim().lines() {
            writeln!(w, "/// {}", line)?;
        }
    }

    writeln!(w, "///")?;
    writeln!(w, "/// - Start bit: {}", signal.start_bit)?;
    writeln!(w, "/// - Signal size: {} bits", signal.signal_size)?;
    writeln!(w, "/// - Factor: {}", signal.factor)?;
    writeln!(w, "/// - Offset: {}", signal.offset)?;
    writeln!(w, "/// - Min: {}", signal.min)?;
    writeln!(w, "/// - Max: {}", signal.max)?;
    writeln!(w, "/// - Byte order: {:?}", signal.byte_order())?;
    writeln!(w, "/// - Value type: {:?}", signal.value_type())?;
    writeln!(w, "/// - Unit: {:?}", signal.unit())?;
    writeln!(w, "/// - Receivers: {}", signal.receivers().join(", "))?;
    writeln!(w, "#[inline(always)]")?;
    writeln!(
        w,
        "pub fn {}(&self) -> {} {{",
        field_name(signal.name()),
        signal_to_rust_type(&signal)
    )?;
    {
        let mut w = PadAdapter::wrap(&mut w);
        signal_from_payload(signal, &mut w)?;
    }
    writeln!(&mut w, "}}")?;
    writeln!(w)?;

    writeln!(&mut w, "/// Set value of {}", signal.name())?;
    writeln!(w, "#[inline(always)]")?;
    writeln!(
        w,
        "pub fn set_{}(&mut self, value: {}) -> Result<(), CanError> {{",
        field_name(signal.name()),
        signal_to_rust_type(&signal)
    )?;
    {
        let mut w = PadAdapter::wrap(&mut w);
        signal_to_payload(signal, &mut w)?;
    }
    writeln!(&mut w, "}}")?;
    writeln!(w)?;

    Ok(())
}

fn signal_from_payload(signal: &Signal, mut w: impl Write) -> Result<()> {
    let read_fn = match signal.byte_order() {
        can_dbc::ByteOrder::LittleEndian => format!(
            "{typ}::unpack_le_bits(&self.raw, {start}, {size})",
            typ = signal_to_rust_int(signal),
            start = signal.start_bit,
            size = signal.signal_size,
        ),
        can_dbc::ByteOrder::BigEndian => format!(
            "{typ}::unpack_be_bits(&self.raw, ({start} - ({size} - 1)), {size})",
            typ = signal_to_rust_int(signal),
            start = signal.start_bit,
            size = signal.signal_size,
        ),
    };

    writeln!(&mut w, r#"let signal = {};"#, read_fn)?;
    writeln!(&mut w)?;

    if signal.signal_size == 1 {
        writeln!(&mut w, "signal == 1")?;
    } else if signal_is_float_in_rust(signal) {
        // Scaling is always done on floats
        writeln!(&mut w, "let factor = {}_f64;", signal.factor)?;
        writeln!(&mut w, "let offset = {}_f64;", signal.offset)?;
        writeln!(&mut w, "(signal as f64) * factor + offset")?;
    } else {
        writeln!(&mut w, "signal")?;
    }
    Ok(())
}

fn signal_to_payload(signal: &Signal, mut w: impl Write) -> Result<()> {
    if signal.signal_size == 1 {
        // Map boolean to byte so we can pack it
        writeln!(&mut w, "let value = value as u8;")?;
    } else if signal_is_float_in_rust(signal) {
        // Massage value into an int
        writeln!(&mut w, "let factor = {}_f64;", signal.factor)?;
        writeln!(&mut w, "let offset = {}_f64;", signal.offset)?;
        writeln!(
            &mut w,
            "let value = ((value  - offset) / factor) as {};",
            signal_to_rust_int(signal)
        )?;
        writeln!(&mut w)?;
    }

    writeln!(&mut w, "let start_bit = {};", signal.start_bit)?;
    writeln!(&mut w, "let bits = {};", signal.signal_size)?;
    let endianness = match signal.byte_order() {
        can_dbc::ByteOrder::LittleEndian => "le",
        can_dbc::ByteOrder::BigEndian => "be",
    };

    writeln!(
        &mut w,
        r#"value.pack_{}_bits(&mut self.raw, start_bit, bits);"#,
        endianness
    )?;

    writeln!(&mut w, "Ok(())")?;
    Ok(())
}

fn signal_to_rust_int(signal: &Signal) -> String {
    let sign = match signal.value_type() {
        can_dbc::ValueType::Signed => "i",
        can_dbc::ValueType::Unsigned => "u",
    };

    let size = match *signal.signal_size() {
        n if n <= 8 => "8",
        n if n <= 16 => "16",
        n if n <= 32 => "32",
        _ => "64",
    };

    format!("{}{}", sign, size)
}

#[allow(clippy::float_cmp)]
fn signal_is_float_in_rust(signal: &Signal) -> bool {
    *signal.offset() != 0.0 || *signal.factor() != 1.0
}

fn signal_to_rust_type(signal: &Signal) -> String {
    if signal.signal_size == 1 {
        String::from("bool")
    } else if signal_is_float_in_rust(signal) {
        // If there is any scaling needed, go for double precision
        String::from("f64")
    } else {
        signal_to_rust_int(signal)
    }
}

pub fn type_name(x: &str) -> String {
    x.to_camel_case()
}

pub fn field_name(x: &str) -> String {
    x.to_snake_case()
}

pub fn to_enum_name(message_id: MessageId, signal_name: &str) -> String {
    format!("{}{}", &signal_name.to_camel_case(), message_id.0)
}