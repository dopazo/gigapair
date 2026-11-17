//! Tables, progress lines and the terminal QR code.

use gigapair_core::api::same_network_hint;
use gigapair_core::types::{
    ConnKind, Contact, Direction, TransferDetails, TransferInfo, TransferStatus, format_bytes,
    format_duration, short_id,
};

use crate::strings as s;

/// The pairing code as a QR made of Unicode half blocks.
pub fn render_qr(code: &str) -> Option<String> {
    let qr = qrcode::QrCode::with_error_correction_level(code.to_uppercase(), qrcode::EcLevel::L)
        .ok()?;
    Some(
        qr.render::<qrcode::render::unicode::Dense1x2>()
            .dark_color(qrcode::render::unicode::Dense1x2::Light)
            .light_color(qrcode::render::unicode::Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
    )
}

pub fn print_contacts(list: &[Contact]) {
    let width = list
        .iter()
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or(4)
        .max(4);
    println!(
        "{:width$}  {:8}  {:11}  NODE ID",
        "NAME", "STATUS", "AUTO-ACCEPT"
    );
    for c in list {
        println!(
            "{:width$}  {:8}  {:11}  {}",
            c.name,
            if c.online { "online" } else { "offline" },
            if c.auto_accept { "on" } else { "off" },
            c.node_id
        );
    }
}

fn arrow(t: &TransferInfo) -> String {
    match t.direction {
        Direction::Send => format!("→ {}", t.peer_name),
        Direction::Receive => format!("← {}", t.peer_name),
    }
}

pub fn print_transfers(list: &[TransferInfo]) {
    for t in list {
        let conn = t.conn.as_ref().map(|c| c.label()).unwrap_or_default();
        let mut line = format!(
            "{}  {}  {}  {}  {}/{} ({:.1}%)",
            short_id(&t.id),
            arrow(t),
            t.label,
            t.status.label(),
            format_bytes(t.done_bytes),
            format_bytes(t.total_bytes),
            t.percent()
        );
        if t.speed_bps > 0 {
            line.push_str(&format!("  {}/s", format_bytes(t.speed_bps)));
        }
        if let Some(eta) = t.eta_secs {
            line.push_str(&format!("  ETA {}", format_duration(eta)));
        }
        if !conn.is_empty() {
            line.push_str(&format!("  [{conn}]"));
        }
        println!("{line}");
        if let Some(c) = &t.conn
            && c.kind == ConnKind::Relay
            && c.same_network
        {
            println!("          {}", same_network_hint(&t.peer_name));
        }
        if let Some(note) = &t.note {
            println!("          {note}");
        }
        if let Some(err) = &t.error
            && t.status != TransferStatus::Completed
        {
            println!("          {err}");
        }
    }
}

pub fn progress_line(t: &TransferInfo) -> String {
    let mut line = format!(
        "{:>5.1}%  {} / {}  {}",
        t.percent(),
        format_bytes(t.done_bytes),
        format_bytes(t.total_bytes),
        t.status.label()
    );
    if t.speed_bps > 0 {
        line.push_str(&format!("  {}/s", format_bytes(t.speed_bps)));
    }
    if let Some(eta) = t.eta_secs {
        line.push_str(&format!("  ETA {}", format_duration(eta)));
    }
    if let Some(c) = &t.conn {
        line.push_str(&format!("  [{}]", c.label()));
    }
    if let Some(note) = &t.note {
        line.push_str(&format!("  {note}"));
    }
    line
}

/// `gigapair show`: the transfer's line, where it is, and its files.
pub fn print_details(d: &TransferDetails) {
    print_transfers(std::slice::from_ref(&d.transfer));
    if d.locations.is_empty() {
        println!("{}", s::NO_LOCATIONS);
    } else {
        println!("{}", s::LOCATIONS);
        for l in &d.locations {
            println!("  {l}");
        }
    }
    println!("{}", s::FILES);
    for f in &d.files {
        println!(
            "  {} {}  {}",
            if f.done { "✓" } else { "·" },
            f.path,
            format_bytes(f.size)
        );
    }
    if d.more_files > 0 {
        println!("{}", s::more_files(d.more_files));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn qr_renders() {
        let qr = super::render_qr("gp1-abcd-efgh").unwrap();
        assert!(qr.lines().count() > 10);
    }
}
