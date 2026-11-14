//! The table reply shape of list queries (docs/file-format.md, "Table
//! replies"; extension-ABI design §2.4): columns named once, then one row
//! per record, columns in the record's declared field order. Replies are
//! not files: they use the command wire's units and may hold `null` for a
//! value a record leaves out. The queries themselves are ABI slice S2c.
use super::records::RecordOrder;
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `{"revision", "columns", "rows", "more"}`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Table {
    /// The document revision the rows were read at.
    pub revision: u64,
    pub columns: Vec<String>,
    /// One row per record, one value per column; `null` where the record
    /// leaves the value out.
    pub rows: Vec<Vec<Value>>,
    /// Matching records after the last row that this reply leaves out;
    /// 0 when the reply is complete.
    pub more: u64,
}

impl Table {
    /// A table of `records` (each a JSON object of one record type) in the
    /// declared order of `order`. With `fields`, exactly those columns, in
    /// declared order whatever order they were asked in; a field the
    /// record does not declare is refused by name. Without, every declared
    /// column that at least one record has.
    pub fn from_records(
        revision: u64,
        order: &RecordOrder,
        fields: Option<&[&str]>,
        records: &[Value],
        more: u64,
    ) -> Result<Self> {
        if let Some(unknown) = fields
            .unwrap_or_default()
            .iter()
            .find(|f| !order.fields.contains(f))
        {
            return Err(Error::Invalid(format!(
                "unknown field \"{unknown}\"; the fields are {}",
                order.fields.join(", ")
            )));
        }
        let columns: Vec<&str> = order
            .fields
            .iter()
            .copied()
            .filter(|field| match fields {
                Some(asked) => asked.contains(field),
                None => records.iter().any(|r| r.get(field).is_some()),
            })
            .collect();
        let rows = records
            .iter()
            .map(|record| {
                columns
                    .iter()
                    .map(|c| record.get(c).cloned().unwrap_or(Value::Null))
                    .collect()
            })
            .collect();
        Ok(Self {
            revision,
            columns: columns.into_iter().map(str::to_owned).collect(),
            rows,
            more,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::records::TRACKS;
    use serde_json::json;

    type Fallible = std::result::Result<(), Box<dyn std::error::Error>>;

    fn order() -> Result<&'static RecordOrder> {
        TRACKS
            .record(None)
            .ok_or_else(|| Error::Invalid("no track order".into()))
    }

    #[test]
    fn rows_follow_the_declared_order() -> Fallible {
        let records = [
            json!({"end": [1, 0], "id": 4, "layer": "top", "net": 6, "start": [0, 0], "width": 0.3}),
            json!({"end": [2, 0], "id": 5, "layer": "top", "locked": true, "net": 6, "start": [1, 0], "width": 0.3}),
        ];
        let table = Table::from_records(41, order()?, None, &records, 3)?;
        assert_eq!(
            serde_json::to_string(&table)?,
            r#"{"revision":41,"columns":["id","net","layer","width","start","end","locked"],"rows":[[4,6,"top",0.3,[0,0],[1,0],null],[5,6,"top",0.3,[1,0],[2,0],true]],"more":3}"#
        );
        let narrow = Table::from_records(41, order()?, Some(&["layer", "id"]), &records, 0)?;
        assert_eq!(narrow.columns, ["id", "layer"]);
        assert_eq!(
            narrow.rows,
            [vec![json!(4), json!("top")], vec![json!(5), json!("top")]]
        );
        let refused = Table::from_records(41, order()?, Some(&["colour"]), &records, 0);
        assert!(refused.is_err_and(|e| e.to_string().contains("\"colour\"")));
        Ok(())
    }
}
