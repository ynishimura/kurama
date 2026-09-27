//! Stream Arrow cells into bounded JSON buffers, preserving nested structure and numeric precision.
use super::{connection::Connection, handles::is_unsigned_hugeint};
use crate::domain::types::dataset::{DataColumn, DataError, DataLimits, DataResult};
use arrow::{array::*, datatypes::*, util::display::array_value_to_string};
use base64::{display::Base64Display, engine::general_purpose::STANDARD};
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};
use serde_json::Value;
use std::io::{self, Write};

/// Rows of a query kurama wrote itself, refused when the display limits cut
/// them short.
///
/// An internal query answers a question with one right answer -- how many row
/// groups, how many bytes moved -- and a limit meant for what a person sees on
/// screen must not silently shorten it. The caller gets the truncation as an
/// error it cannot ignore instead of rows it cannot tell apart from complete
/// ones.
pub fn collect_complete(
    connection: &Connection,
    sql: &str,
    limits: &DataLimits,
) -> Result<Vec<Vec<Value>>, DataError> {
    let result = collect(connection, sql, limits)?;
    if result.truncated {
        return Err(DataError::Incomplete);
    }
    Ok(result.rows)
}

/// An engine integer arrives as text so a 64-bit value survives a reader that
/// would round it. `None` is "no number here", which is never the same answer
/// as zero.
pub fn number(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| value.as_str()?.parse().ok())
}

pub fn collect(
    connection: &Connection,
    sql: &str,
    limits: &DataLimits,
) -> Result<DataResult, DataError> {
    let mut result = DataResult {
        columns: vec![],
        rows: vec![],
        row_count: 0,
        truncated: false,
        stop_reason: None,
        result_bytes: 2,
    };
    let schema = connection.stream(sql, |batch| {
        for row in 0..batch.len() {
            if result.rows.len() == limits.max_rows {
                result.stop_reason = Some("max_rows".into());
                return Ok(false);
            }
            let separator = usize::from(!result.rows.is_empty());
            let remaining = limits
                .max_result_bytes
                .saturating_sub(result.result_bytes + separator);
            let Some((values, bytes)) = row_values(batch, row, remaining)? else {
                result.stop_reason = Some("max_result_bytes".into());
                return Ok(false);
            };
            result.result_bytes += bytes + separator;
            result.rows.push(values);
        }
        Ok(true)
    })?;
    let schema = arrow::datatypes::Schema::try_from(&schema)
        .map_err(|_| DataError::Engine("result schema"))?;
    result.columns = schema
        .fields()
        .iter()
        .map(|field| DataColumn {
            name: field.name().clone(),
            data_type: if is_unsigned_hugeint(field) {
                "UHUGEINT".into()
            } else {
                field.data_type().to_string()
            },
        })
        .collect();
    result.row_count = result.rows.len();
    result.truncated = result.stop_reason.is_some();
    Ok(result)
}

/// No Arrow value is first copied into an unbounded String or Value. The row
/// and each nested JSON string stop writing as soon as their remaining budget
/// is exhausted. Only a completed bounded row becomes owned JSON values.
fn row_values(
    batch: &StructArray,
    row: usize,
    budget: usize,
) -> Result<Option<(Vec<Value>, usize)>, DataError> {
    let mut output = BoundedBytes::new(budget);
    if output.write_all(b"[").is_err() {
        return Ok(None);
    }
    for (index, (field, array)) in batch.fields().iter().zip(batch.columns()).enumerate() {
        if index > 0 && output.write_all(b",").is_err() {
            return Ok(None);
        }
        let cell = JsonCell {
            array: array.as_ref(),
            field,
            row,
        };
        if cell.is_nested() && !cell.is_null() {
            let mut inner = BoundedBytes::new(output.remaining());
            if !write_json(&mut inner, &cell)? {
                return Ok(None);
            }
            let text = std::str::from_utf8(&inner.bytes).expect("JSON serialization is UTF-8");
            if !write_json(&mut output, text)? {
                return Ok(None);
            }
        } else if !write_json(&mut output, &cell)? {
            return Ok(None);
        }
    }
    if output.write_all(b"]").is_err() {
        return Ok(None);
    }
    let bytes = output.bytes.len();
    let values =
        serde_json::from_slice(&output.bytes).map_err(|_| DataError::Engine("cell conversion"))?;
    Ok(Some((values, bytes)))
}

fn write_json(
    output: &mut BoundedBytes,
    value: &(impl Serialize + ?Sized),
) -> Result<bool, DataError> {
    match serde_json::to_writer(&mut *output, value) {
        Ok(()) => Ok(true),
        Err(_) if output.exceeded => Ok(false),
        Err(_) => Err(DataError::Engine("cell conversion")),
    }
}

struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl BoundedBytes {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        }
    }
    fn remaining(&self) -> usize {
        self.limit - self.bytes.len()
    }
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining() {
            self.exceeded = true;
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() {
            let capacity = required
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.limit);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct JsonCell<'a> {
    array: &'a dyn Array,
    field: &'a Field,
    row: usize,
}
impl<'a> JsonCell<'a> {
    fn is_null(&self) -> bool {
        match self.array.data_type() {
            DataType::Union(_, _) => self.union_member().is_null(),
            _ => self.array.is_null(self.row),
        }
    }
    fn union_member(&self) -> Self {
        let array = self.array.as_any().downcast_ref::<UnionArray>().unwrap();
        let DataType::Union(fields, _) = self.field.data_type() else {
            unreachable!("union field")
        };
        let id = array.type_id(self.row);
        let field = fields
            .iter()
            .find(|(key, _)| *key == id)
            .expect("union member")
            .1;
        Self {
            array: array.child(id).as_ref(),
            field,
            row: array.value_offset(self.row),
        }
    }
    fn is_nested(&self) -> bool {
        match self.array.data_type() {
            DataType::List(_)
            | DataType::LargeList(_)
            | DataType::FixedSizeList(_, _)
            | DataType::Struct(_)
            | DataType::Map(_, _) => true,
            DataType::Union(_, _) => self.union_member().is_nested(),
            _ => false,
        }
    }
}
impl Serialize for JsonCell<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Self { array, field, row } = *self;
        if self.is_null() {
            return serializer.serialize_none();
        }
        if is_unsigned_hugeint(field) {
            let array = array
                .as_any()
                .downcast_ref::<Decimal128Array>()
                .expect("DuckDB UHUGEINT Arrow storage");
            return serializer.collect_str(&(array.value(row) as u128));
        }
        match array.data_type() {
            DataType::Boolean => serializer.serialize_bool(
                array
                    .as_any()
                    .downcast_ref::<BooleanArray>()
                    .unwrap()
                    .value(row),
            ),
            DataType::Utf8 => serializer.serialize_str(
                array
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap()
                    .value(row),
            ),
            DataType::LargeUtf8 => serializer.serialize_str(
                array
                    .as_any()
                    .downcast_ref::<LargeStringArray>()
                    .unwrap()
                    .value(row),
            ),
            DataType::Utf8View => serializer.serialize_str(
                array
                    .as_any()
                    .downcast_ref::<StringViewArray>()
                    .unwrap()
                    .value(row),
            ),
            DataType::Binary => serializer.collect_str(&Base64Display::new(
                array
                    .as_any()
                    .downcast_ref::<BinaryArray>()
                    .unwrap()
                    .value(row),
                &STANDARD,
            )),
            DataType::LargeBinary => serializer.collect_str(&Base64Display::new(
                array
                    .as_any()
                    .downcast_ref::<LargeBinaryArray>()
                    .unwrap()
                    .value(row),
                &STANDARD,
            )),
            DataType::BinaryView => serializer.collect_str(&Base64Display::new(
                array
                    .as_any()
                    .downcast_ref::<BinaryViewArray>()
                    .unwrap()
                    .value(row),
                &STANDARD,
            )),
            DataType::FixedSizeBinary(_) => serializer.collect_str(&Base64Display::new(
                array
                    .as_any()
                    .downcast_ref::<FixedSizeBinaryArray>()
                    .unwrap()
                    .value(row),
                &STANDARD,
            )),
            DataType::List(child) => serialize_list(
                array
                    .as_any()
                    .downcast_ref::<ListArray>()
                    .unwrap()
                    .value(row),
                child,
                serializer,
            ),
            DataType::LargeList(child) => serialize_list(
                array
                    .as_any()
                    .downcast_ref::<LargeListArray>()
                    .unwrap()
                    .value(row),
                child,
                serializer,
            ),
            DataType::FixedSizeList(child, _) => serialize_list(
                array
                    .as_any()
                    .downcast_ref::<FixedSizeListArray>()
                    .unwrap()
                    .value(row),
                child,
                serializer,
            ),
            DataType::Struct(_) => {
                let array = array.as_any().downcast_ref::<StructArray>().unwrap();
                let mut object = serializer.serialize_map(Some(array.num_columns()))?;
                for (field, array) in array.fields().iter().zip(array.columns()) {
                    object.serialize_entry(
                        field.name(),
                        &JsonCell {
                            array: array.as_ref(),
                            field,
                            row,
                        },
                    )?;
                }
                object.end()
            }
            DataType::Map(entries, _) => serialize_list(
                std::sync::Arc::new(
                    array
                        .as_any()
                        .downcast_ref::<MapArray>()
                        .unwrap()
                        .value(row),
                ),
                entries,
                serializer,
            ),
            DataType::Union(_, _) => self.union_member().serialize(serializer),
            DataType::Dictionary(key, _) => match key.as_ref() {
                DataType::Int8 => serialize_dictionary::<Int8Type, S>(array, row, serializer),
                DataType::Int16 => serialize_dictionary::<Int16Type, S>(array, row, serializer),
                DataType::Int32 => serialize_dictionary::<Int32Type, S>(array, row, serializer),
                DataType::Int64 => serialize_dictionary::<Int64Type, S>(array, row, serializer),
                DataType::UInt8 => serialize_dictionary::<UInt8Type, S>(array, row, serializer),
                DataType::UInt16 => serialize_dictionary::<UInt16Type, S>(array, row, serializer),
                DataType::UInt32 => serialize_dictionary::<UInt32Type, S>(array, row, serializer),
                DataType::UInt64 => serialize_dictionary::<UInt64Type, S>(array, row, serializer),
                _ => Err(serde::ser::Error::custom("unsupported dictionary key")),
            },
            _ => {
                // Only fixed-size scalar types reach this branch. Keep Arrow's
                // fallible conversion: a fallible Display in collect_str can
                // panic inside serde_json on a non-I/O formatting error.
                let text = array_value_to_string(array, row).map_err(serde::ser::Error::custom)?;
                serializer.serialize_str(&text)
            }
        }
    }
}

fn serialize_dictionary<K: ArrowDictionaryKeyType, S: Serializer>(
    array: &dyn Array,
    row: usize,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let array = array.as_any().downcast_ref::<DictionaryArray<K>>().unwrap();
    let values = array.values();
    let field = Field::new("dictionary value", values.data_type().clone(), true);
    JsonCell {
        array: values.as_ref(),
        field: &field,
        row: array.key(row).expect("non-null dictionary key"),
    }
    .serialize(serializer)
}

fn serialize_list<S: Serializer>(
    array: ArrayRef,
    field: &Field,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut sequence = serializer.serialize_seq(Some(array.len()))?;
    for row in 0..array.len() {
        sequence.serialize_element(&JsonCell {
            array: array.as_ref(),
            field,
            row,
        })?;
    }
    sequence.end()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
        sync::Arc,
    };

    // Measure only the calling test thread and only after Arrow fixture setup.
    // This detects a full-cell clone before the bounded writer as well as a
    // writer that starts growing past the limit.
    thread_local! {
        static MEASURE: Cell<bool> = const { Cell::new(false) };
        static LARGEST: Cell<usize> = const { Cell::new(0) };
    }
    struct MeasuredAllocator;
    #[global_allocator]
    static ALLOCATOR: MeasuredAllocator = MeasuredAllocator;
    fn record_allocation(size: usize) {
        let _ = MEASURE.try_with(|measure| {
            if measure.get() {
                let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
            }
        });
    }
    unsafe impl GlobalAlloc for MeasuredAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record_allocation(layout.size());
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record_allocation(layout.size());
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            unsafe { System.dealloc(pointer, layout) }
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            record_allocation(size);
            unsafe { System.realloc(pointer, layout, size) }
        }
    }

    #[test]
    fn data_result_budget_counts_json_escaping_and_row_brackets() {
        let batch = StructArray::from(vec![(
            Arc::new(Field::new("text", DataType::Utf8, false)),
            Arc::new(StringArray::from(vec!["\"\n日本語"])) as ArrayRef,
        )]);
        let expected = serde_json::to_vec(&vec![Value::String("\"\n日本語".into())]).unwrap();
        assert!(row_values(&batch, 0, expected.len() - 1).unwrap().is_none());
        let (values, bytes) = row_values(&batch, 0, expected.len()).unwrap().unwrap();
        assert_eq!(serde_json::to_vec(&values).unwrap(), expected);
        assert_eq!(bytes, expected.len());
    }

    #[test]
    fn data_result_byte_writer_never_allocates_past_its_budget() {
        let large = "a".repeat(1_000_000);
        let mut writer = BoundedBytes::new(100);
        assert!(!write_json(&mut writer, &large).unwrap());
        assert!(writer.bytes.capacity() <= 100);
        assert!(writer.bytes.len() <= 100);
        let batch = StructArray::from(vec![(
            Arc::new(Field::new("large", DataType::Utf8, false)),
            Arc::new(StringArray::from(vec![large])) as ArrayRef,
        )]);
        LARGEST.set(0);
        MEASURE.set(true);
        let result = row_values(&batch, 0, 100);
        MEASURE.set(false);
        let largest = LARGEST.get();
        assert!(result.unwrap().is_none());
        assert!(
            largest <= 1_024,
            "a cell bypassed the 100-byte output budget with a {largest}-byte allocation"
        );
    }
}
