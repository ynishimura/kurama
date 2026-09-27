//! Catch all C++ exceptions around Arrow APIs that allocate outside DuckDB's own try blocks.
#include "duckdb.h"

extern "C" duckdb_arrow_options kurama_arrow_options(duckdb_result *result) noexcept {
    try { return duckdb_result_get_arrow_options(result); }
    catch (...) { return nullptr; }
}

extern "C" duckdb_logical_type kurama_column_type(duckdb_result *result, idx_t index) noexcept {
    try { return duckdb_column_logical_type(result, index); }
    catch (...) { return nullptr; }
}

extern "C" duckdb_logical_type kurama_child_type(duckdb_logical_type type, idx_t index) noexcept {
    try {
        switch (duckdb_get_type_id(type)) {
        case DUCKDB_TYPE_LIST: return duckdb_list_type_child_type(type);
        case DUCKDB_TYPE_ARRAY: return duckdb_array_type_child_type(type);
        case DUCKDB_TYPE_STRUCT: return duckdb_struct_type_child_type(type, index);
        case DUCKDB_TYPE_MAP: return index == 0 ? duckdb_map_type_key_type(type) : duckdb_map_type_value_type(type);
        case DUCKDB_TYPE_UNION: return duckdb_union_type_member_type(type, index);
        default: return nullptr;
        }
    } catch (...) { return nullptr; }
}

extern "C" bool kurama_arrow_schema(duckdb_arrow_options options, duckdb_logical_type *types,
                                     const char **names, idx_t count, ArrowSchema *schema) noexcept {
    try {
        auto error = duckdb_to_arrow_schema(options, types, names, count, schema);
        const bool success = !duckdb_error_data_has_error(error);
        duckdb_destroy_error_data(&error);
        return success;
    } catch (...) { return false; }
}

extern "C" bool kurama_arrow_array(duckdb_arrow_options options, duckdb_data_chunk chunk,
                                    ArrowArray *array) noexcept {
    try {
        auto error = duckdb_data_chunk_to_arrow(options, chunk, array);
        const bool success = !duckdb_error_data_has_error(error);
        duckdb_destroy_error_data(&error);
        return success;
    } catch (...) { return false; }
}
