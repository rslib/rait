use crate::Table;
use parquet::basic::{ConvertedType, LogicalType, Type as Physical};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::reader::RowIter;
use parquet::schema::types::Type;
use std::fs::File;
use std::path::Path;

pub fn open(path: &Path, sheet: String) -> Result<Table, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let reader = SerializedFileReader::new(file).map_err(|e| e.to_string())?;
    let meta = reader.metadata().file_metadata();
    let fields = meta.schema_descr().root_schema().get_fields();
    let header = fields.iter().map(|f| f.name().to_string()).collect();
    let types = fields.iter().map(|f| type_name(f).to_string()).collect();
    let total_rows = meta.num_rows().max(0) as u64;
    let rows = RowIter::from_file_into(Box::new(reader)).map(|row| {
        row.map(|row| {
            row.get_column_iter()
                .map(|(_, field)| field.to_json_value())
                .collect()
        })
        .map_err(|e| e.to_string())
    });
    Ok(Table {
        sheet,
        header,
        total_rows,
        types: Some(types),
        rows: Box::new(rows),
        warnings: vec![],
    })
}

fn type_name(field: &Type) -> &'static str {
    if !field.is_primitive() {
        return "nested";
    }
    let info = field.get_basic_info();
    match info.logical_type_ref() {
        Some(LogicalType::String | LogicalType::Enum | LogicalType::Json | LogicalType::Uuid) => {
            return "string";
        }
        Some(LogicalType::Date) => return "date",
        Some(LogicalType::Timestamp { .. }) => return "datetime",
        Some(LogicalType::Time { .. }) => return "time",
        Some(LogicalType::Decimal { .. }) => return "decimal",
        Some(LogicalType::Integer { .. }) => return "int",
        _ => {}
    }
    if info.converted_type() == ConvertedType::UTF8 {
        return "string";
    }
    match field.get_physical_type() {
        Physical::BOOLEAN => "bool",
        Physical::INT32 | Physical::INT64 => "int",
        Physical::INT96 => "datetime",
        Physical::FLOAT | Physical::DOUBLE => "float",
        Physical::BYTE_ARRAY | Physical::FIXED_LEN_BYTE_ARRAY => "binary",
    }
}
