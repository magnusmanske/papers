use mysql as my;

#[derive(Debug, Clone)]
pub struct SourceMDcommand {
    pub id: i64,
    pub batch_id: i64,
    pub serial_number: i64,
    pub mode: String,
    pub identifier: String,
    pub status: String,
    pub note: String,
    pub q: String,
    pub auto_escalate: bool,
}

impl SourceMDcommand {
    pub fn new_from_row(row: my::Row) -> Self {
        Self {
            id: SourceMDcommand::rowvalue_as_i64(&row["id"]),
            batch_id: SourceMDcommand::rowvalue_as_i64(&row["batch_id"]),
            serial_number: SourceMDcommand::rowvalue_as_i64(&row["serial_number"]),
            mode: SourceMDcommand::rowvalue_as_string(&row["mode"]),
            identifier: SourceMDcommand::rowvalue_as_string(&row["identifier"]),
            status: SourceMDcommand::rowvalue_as_string(&row["status"]),
            note: SourceMDcommand::rowvalue_as_string(&row["note"]),
            q: SourceMDcommand::rowvalue_as_string(&row["q"]),
            auto_escalate: SourceMDcommand::rowvalue_as_i64(&row["auto_escalate"]) == 1,
        }
    }

    fn rowvalue_as_i64(v: &my::Value) -> i64 {
        match v {
            my::Value::Int(x) => *x,
            _ => 0,
        }
    }

    fn rowvalue_as_string(v: &my::Value) -> String {
        match v {
            my::Value::Bytes(x) => String::from_utf8_lossy(x).to_string(),
            _ => String::from(""),
        }
    }
}

#[cfg(test)]
mod tests {
    //use super::*;
    //use mediawiki::api::Api;

    /*
    TODO:
    pub fn new_from_row(row: my::Row) -> Self {
    fn rowvalue_as_i64(v: &my::Value) -> i64 {
    fn rowvalue_as_string(v: &my::Value) -> String {
    */
}
