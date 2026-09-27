//! Owned DuckDB result handles and a noexcept Arrow bridge with unsigned integer metadata.
use crate::domain::types::dataset::{DataError, InvalidInput};
use arrow::{
    datatypes::{DataType, Field, Schema, UnionFields},
    ffi::FFI_ArrowSchema,
};
use libduckdb_sys as ffi;
use std::{ffi::c_char, sync::Arc};
use zeroize::Zeroizing;

unsafe extern "C" {
    fn kurama_arrow_options(result: *mut ffi::duckdb_result) -> ffi::duckdb_arrow_options;
    fn kurama_column_type(result: *mut ffi::duckdb_result, index: u64) -> ffi::duckdb_logical_type;
    fn kurama_child_type(ty: ffi::duckdb_logical_type, index: u64) -> ffi::duckdb_logical_type;
    fn kurama_arrow_schema(
        options: ffi::duckdb_arrow_options,
        types: *mut ffi::duckdb_logical_type,
        names: *mut *const c_char,
        count: u64,
        schema: *mut ffi::ArrowSchema,
    ) -> bool;
    pub(super) fn kurama_arrow_array(
        options: ffi::duckdb_arrow_options,
        chunk: ffi::duckdb_data_chunk,
        array: *mut ffi::ArrowArray,
    ) -> bool;
}

macro_rules! owned_handle {
    ($name:ident, $raw:ty, $destroy:path) => {
        pub(super) struct $name(pub(super) $raw);
        impl Drop for $name {
            fn drop(&mut self) {
                // SAFETY: this is the sole owner, including null handles on failures.
                unsafe { $destroy(&mut self.0) };
            }
        }
    };
}
owned_handle!(Config, ffi::duckdb_config, ffi::duckdb_destroy_config);
owned_handle!(
    Statement,
    ffi::duckdb_prepared_statement,
    ffi::duckdb_destroy_prepare
);
owned_handle!(
    Extracted,
    ffi::duckdb_extracted_statements,
    ffi::duckdb_destroy_extracted
);
owned_handle!(ResultSet, ffi::duckdb_result, ffi::duckdb_destroy_result);
owned_handle!(
    Chunk,
    ffi::duckdb_data_chunk,
    ffi::duckdb_destroy_data_chunk
);
owned_handle!(
    ArrowOptions,
    ffi::duckdb_arrow_options,
    ffi::duckdb_destroy_arrow_options
);
owned_handle!(
    LogicalType,
    ffi::duckdb_logical_type,
    ffi::duckdb_destroy_logical_type
);

impl ResultSet {
    pub(super) fn new() -> Self {
        // SAFETY: DuckDB accepts a zeroed result as an out parameter and in destroy.
        Self(unsafe { std::mem::zeroed() })
    }
}

/// The extra NUL-terminated allocation must be erased too: statements can
/// contain temporary credentials. DuckDB's own internal copies are engine-owned.
pub(super) fn cstring(value: &str) -> Result<Zeroizing<Vec<u8>>, DataError> {
    if value.as_bytes().contains(&0) {
        return Err(InvalidInput::NulByte.into());
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(value.len() + 1));
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(0);
    Ok(bytes)
}

pub(super) fn result_schema(
    result: &mut ResultSet,
) -> Result<(FFI_ArrowSchema, ArrowOptions), DataError> {
    // SAFETY: result remains alive for options and every source logical type.
    // Allocating C++ calls use the bridge, whose catch also covers allocations
    // before DuckDB's internal try blocks.
    unsafe {
        let count = ffi::duckdb_column_count(&mut result.0);
        let types = (0..count)
            .map(|index| {
                let ty = LogicalType(kurama_column_type(&mut result.0, index));
                if ty.0.is_null() {
                    Err(DataError::Engine("result schema"))
                } else {
                    Ok(ty)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut raw_types: Vec<_> = types.iter().map(|ty| ty.0).collect();
        let mut names: Vec<_> = (0..count)
            .map(|index| ffi::duckdb_column_name(&mut result.0, index))
            .collect();
        let options = ArrowOptions(kurama_arrow_options(&mut result.0));
        if options.0.is_null() || names.iter().any(|name| name.is_null()) {
            return Err(DataError::Engine("result schema"));
        }
        let mut schema = FFI_ArrowSchema::empty();
        if !kurama_arrow_schema(
            options.0,
            raw_types.as_mut_ptr(),
            names.as_mut_ptr(),
            count,
            (&mut schema as *mut FFI_ArrowSchema).cast(),
        ) {
            return Err(DataError::Engine("result schema"));
        }
        let schema = Schema::try_from(&schema).map_err(|_| DataError::Engine("result schema"))?;
        let fields = schema
            .fields()
            .iter()
            .zip(&types)
            .map(|(field, ty)| annotate_unsigned(field, ty))
            .collect::<Result<Vec<_>, _>>()?;
        let schema = FFI_ArrowSchema::try_from(&Schema::new(fields))
            .map_err(|_| DataError::Engine("result schema"))?;
        Ok((schema, options))
    }
}

const UNSIGNED_HUGEINT: &str = "kurama.duckdb.uhugeint";

pub(super) fn is_unsigned_hugeint(field: &Field) -> bool {
    field.metadata().contains_key(UNSIGNED_HUGEINT)
}

fn annotate_unsigned(field: &Field, ty: &LogicalType) -> Result<Field, DataError> {
    let mut field = field.clone();
    // SAFETY: logical types are owned until recursion returns.
    if unsafe { ffi::duckdb_get_type_id(ty.0) } == ffi::DUCKDB_TYPE_DUCKDB_TYPE_UHUGEINT {
        let mut metadata = field.metadata().clone();
        metadata.insert(UNSIGNED_HUGEINT.into(), "true".into());
        return Ok(field.with_metadata(metadata));
    }
    let child = |field: &Field, index| {
        let ty = LogicalType(unsafe { kurama_child_type(ty.0, index) });
        if ty.0.is_null() {
            return Err(DataError::Engine("result schema"));
        }
        annotate_unsigned(field, &ty).map(Arc::new)
    };
    let data_type = match field.data_type() {
        DataType::List(f) => DataType::List(child(f, 0)?),
        DataType::LargeList(f) => DataType::LargeList(child(f, 0)?),
        DataType::FixedSizeList(f, size) => DataType::FixedSizeList(child(f, 0)?, *size),
        DataType::Struct(fields) => DataType::Struct(
            fields
                .iter()
                .enumerate()
                .map(|(i, f)| child(f, i as u64))
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        ),
        DataType::Map(entries, ordered) => {
            let DataType::Struct(fields) = entries.data_type() else {
                return Err(DataError::Engine("result schema"));
            };
            let fields = fields
                .iter()
                .enumerate()
                .map(|(i, f)| child(f, i as u64))
                .collect::<Result<Vec<_>, _>>()?;
            DataType::Map(
                Arc::new(
                    entries
                        .as_ref()
                        .clone()
                        .with_data_type(DataType::Struct(fields.into())),
                ),
                *ordered,
            )
        }
        DataType::Union(fields, mode) => {
            let fields = fields
                .iter()
                .enumerate()
                .map(|(index, (id, f))| child(f, index as u64).map(|f| (id, f)))
                .collect::<Result<Vec<_>, _>>()?;
            DataType::Union(UnionFields::from_iter(fields), *mode)
        }
        _ => return Ok(field),
    };
    field = field.with_data_type(data_type);
    Ok(field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::ffi::FFI_ArrowArray;
    use std::ptr;

    #[test]
    fn data_engine_arrow_bridge_returns_errors_for_missing_handles() {
        let mut array = FFI_ArrowArray::empty();
        let mut schema = FFI_ArrowSchema::empty();
        // Null input failures exercise bridge cleanup and propagate a result
        // instead of passing an empty Arrow object to from_ffi.
        unsafe {
            assert!(!kurama_arrow_array(
                ptr::null_mut(),
                ptr::null_mut(),
                (&mut array as *mut FFI_ArrowArray).cast()
            ));
            assert!(!kurama_arrow_schema(
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                (&mut schema as *mut FFI_ArrowSchema).cast()
            ));
            assert!(kurama_arrow_options(ptr::null_mut()).is_null());
            assert!(kurama_column_type(ptr::null_mut(), 0).is_null());
        }
    }
}
