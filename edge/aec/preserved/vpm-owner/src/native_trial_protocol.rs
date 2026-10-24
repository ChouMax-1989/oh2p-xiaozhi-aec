fn uuid(value: &str) -> bool {
    value.len() == 36 && value.bytes().enumerate().all(|(i, b)| {
        if [8, 13, 18, 23].contains(&i) { b == b'-' } else { b.is_ascii_hexdigit() }
    })
}
pub fn parse(data: &[u8]) -> Option<(bool, &str)> {
    let text = std::str::from_utf8(data).ok()?;
    let fields: Vec<_> = text.trim_end_matches('\n').split(' ').collect();
    if fields.len() != 4 || fields[0] != "FVNT1" || fields[1] != "SET" || !uuid(fields[3]) { return None; }
    Some((match fields[2] { "1" => true, "0" => false, _ => return None }, fields[3]))
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn protocol_is_bounded_and_revision_required() {
        assert!(parse(b"FVNT1 SET 1 00000000-0000-0000-0000-000000000000\n").unwrap().0);
        assert!(!parse(b"FVNT1 SET 0 00000000-0000-0000-0000-000000000000\n").unwrap().0);
        for bad in ["FVNT1 SET 1 x", "FVNT1 SET 2 00000000-0000-0000-0000-000000000000", "FVNT1 SET 1 00000000-0000-0000-0000-000000000000 extra"] { assert!(parse(bad.as_bytes()).is_none()); }
    }
}
