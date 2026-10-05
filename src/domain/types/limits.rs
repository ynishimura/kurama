//! The bounds external input is cut to before it reaches a person: one place,
//! so the same bound is not written twice.

/// How far the structural views walk, and how much of a value they keep.
///
/// A description and a body are both external input with no size of their
/// own: a self-referential `$ref` or a deeply nested object would otherwise
/// walk forever, and a long scalar would be copied whole to show 48
/// characters of it.
pub struct ShapeLimits {
    /// Levels of nesting `Schema` normalization and `JsonShape` keep. Deeper
    /// levels become `{}` / `Unknown`, which also ends a `$ref` cycle; a chain
    /// of `$ref`s counts toward it on its own.
    pub depth: usize,
    /// Characters of a scalar kept as its sample; the rest is never shown.
    pub sample_chars: usize,
}

pub const SHAPE: ShapeLimits = ShapeLimits {
    depth: 6,
    sample_chars: 48,
};

/// How far a GraphQL type is expanded into the `Schema` of an operation.
///
/// GraphQL object types reference each other everywhere (an issue has an
/// assignee, who has issues): Linear's schema has about 1200 types, and every
/// root field expanded to `SHAPE.depth` would be millions of nodes to list
/// 550 operations. A deeper level stays one `--describe` of a narrower
/// operation, or a query, away.
pub struct GraphQlLimits {
    /// Object levels of a return type kept: the type's own fields, and the
    /// fields of the objects they hold; deeper objects are `{}`.
    pub response_depth: usize,
    /// Input object levels of the arguments kept: an input is what a person
    /// writes, and it is small.
    pub input_depth: usize,
}

pub const GRAPHQL: GraphQlLimits = GraphQlLimits {
    response_depth: 2,
    input_depth: 4,
};

/// How many of the objects an analysis selected are reported one by one.
///
/// A glob may select up to `max_source_objects` files. Listing every one of
/// them, or reading every one's Parquet footer, turns a description into a
/// dump and a thousand range reads; both are cut to the same number and the
/// rest are counted.
pub struct InputListLimits {
    /// Inputs named in `meta.inputs`, and objects a physical summary reads a
    /// footer from. The remainder is reported as an omitted count.
    pub listed: usize,
}

pub const INPUT_LIST: InputListLimits = InputListLimits { listed: 100 };

/// How much one write may be.
///
/// A write runs every statement of one request in one transaction, and holds
/// the locks it takes until the last of them. A request is a change someone
/// wrote down, not a migration, so its length is bounded where the request is
/// read rather than discovered when the database stops answering others.
pub struct WriteLimits {
    /// Statements one `execute` request may carry.
    pub statements: usize,
}

pub const WRITE: WriteLimits = WriteLimits { statements: 50 };

/// What one database call may ask for.
///
/// A deadline is a point in time, not a duration: `Instant + Duration` panics
/// past the end of the clock, so "wait forever" arrives here as an overflow
/// and not as a timeout. A listing page is a page someone reads, so it is
/// bounded where the request is read rather than when the rows arrive.
pub struct DbCallLimits {
    /// The longest deadline a call may set. A day is longer than any statement
    /// a person waits for, and far from where the clock ends.
    pub query_timeout_secs: u64,
    /// Rows one listing page returns when the request does not say.
    pub list_page: usize,
    /// The largest listing page a request may ask for.
    pub max_list_page: usize,
}

pub const DB_CALL: DbCallLimits = DbCallLimits {
    query_timeout_secs: 86_400,
    list_page: 100,
    max_list_page: 1000,
};

/// How much of an API description `kurama api <API> --skill` writes out.
///
/// A Skill is read whole by an agent before every task it matches, so it is
/// a map of the API, not the API: GitHub's description has 1224 operations,
/// and one line each would be a hundred kilobytes nobody reads. What is cut
/// here stays one `--schema OP` / `--ops QUERY` away.
pub struct SkillLimits {
    /// Operations up to this count each get their parameters, body and
    /// response; above it every operation is one index line.
    pub detailed_operations: usize,
    /// Index lines (and tag lines) written before the rest is counted.
    pub listed_operations: usize,
    /// Characters of one summary, help or name list on a line.
    pub line_chars: usize,
    /// Characters of the description's own text.
    pub description_chars: usize,
    /// Enum values shown for one parameter before the rest are counted.
    pub enum_values: usize,
}

pub const SKILL: SkillLimits = SkillLimits {
    detailed_operations: 25,
    listed_operations: 200,
    line_chars: 160,
    description_chars: 800,
    enum_values: 5,
};

/// How much `kurama s3` reads of an object's contents.
///
/// An object is external input of any size: a preview is a page someone
/// reads and a content search is a scan someone waits for, so both are cut
/// where the bytes arrive rather than when memory runs out. These are
/// kurama's own bounds, not AWS's.
pub struct S3ReadLimits {
    /// Bytes one `--preview` shows when `--bytes` does not say.
    pub preview_bytes: u64,
    /// The largest `--bytes` a preview may ask for.
    pub max_preview_bytes: u64,
    /// Compressed bytes a gzip preview transfers from the start.
    pub gzip_transfer_bytes: u64,
    /// Bytes of a binary object shown as hex.
    pub hex_bytes: usize,
    /// Objects one content search lists when `--max-objects` does not say.
    pub search_objects: u64,
    /// Bytes one searched object may transfer, and may decode to.
    pub object_bytes: u64,
    /// Bytes one content search transfers in all, and decodes in all.
    pub total_bytes: u64,
    /// Matching lines one content search reports before it stops.
    pub matches: usize,
    /// Characters of one matching line kept as its excerpt.
    pub excerpt_chars: usize,
}

pub const S3_READ: S3ReadLimits = S3ReadLimits {
    preview_bytes: 64 * 1024,
    max_preview_bytes: 1024 * 1024,
    gzip_transfer_bytes: 1024 * 1024,
    hex_bytes: 64,
    search_objects: 100,
    object_bytes: 8 * 1024 * 1024,
    total_bytes: 50 * 1024 * 1024,
    matches: 1000,
    excerpt_chars: 256,
};

/// What one HTTP request to `kurama mcp --listen` may take.
///
/// The endpoint faces the internet through Funnel, so a scanner holding a
/// connection open or sending a body without end must not hold a task or
/// memory for long. A tool call itself is bounded by `[mcp] call_timeout`.
#[derive(Clone, Copy)]
pub struct McpHttpLimits {
    /// Bytes of a request body; a longer one is `413`.
    pub body_bytes: usize,
    /// Seconds to send the request line and headers, the next request's
    /// included on a kept-alive connection.
    pub header_secs: u64,
    /// Seconds to send the body once the headers are in.
    pub body_secs: u64,
}

pub const MCP_HTTP: McpHttpLimits = McpHttpLimits {
    body_bytes: 1024 * 1024,
    header_secs: 10,
    body_secs: 30,
};

impl InputListLimits {
    /// The prefix reported one by one, and how many were left out.
    ///
    /// Everything cut to this bound goes through here, so the input list and
    /// the Parquet footers cannot stop at different places.
    pub fn split<'a, T>(&self, items: &'a [T]) -> (&'a [T], usize) {
        let listed = items.len().min(self.listed);
        (&items[..listed], items.len() - listed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Destructured without `..` on purpose: a bound added here has to be
    /// given a boundary test below, or this stops compiling. A limit with no
    /// test is a limit nobody has seen take effect.
    #[test]
    fn every_bound_is_pinned_at_its_boundary() {
        let ShapeLimits {
            depth,
            sample_chars,
        } = SHAPE;

        // `json_shape::tests` walks one level past `depth` and expects
        // `Unknown`; `openapi_tests` turns a cycle at `depth` into `{}`.
        assert_eq!(depth, 6, "changing this changes what --describe prints");
        // `json_shape::tests` pins the 48th and 49th character.
        assert_eq!(sample_chars, 48, "a candidate's sample is one line of it");

        let WriteLimits { statements } = WRITE;
        // One statement past the bound is refused before the transaction
        // opens; at the bound it is accepted.
        assert_eq!(statements, 50, "changing this changes what `db` accepts");

        let DbCallLimits {
            query_timeout_secs,
            list_page,
            max_list_page,
        } = DB_CALL;
        // One second past the bound is refused by `DbLimits::validate`; at the
        // bound it is accepted, and the deadline it makes does not overflow.
        assert_eq!(
            query_timeout_secs, 86_400,
            "changing this changes what `db --timeout` accepts"
        );
        // The point of the bound: a deadline is `Instant + Duration`, which
        // panics near the end of the clock. `db_a_deadline_nobody_can_wait_out_
        // is_refused` pins the refusal; this pins that the bound is nowhere
        // near where the addition stops working.
        assert!(
            query_timeout_secs < u64::MAX / 1_000_000_000,
            "a deadline this long is not a timeout but an overflow"
        );
        // `db --schemas` with no --limit returns this many rows, and one past
        // `max_list_page` is refused.
        assert_eq!(
            list_page, 100,
            "changing this changes what one listing page returns"
        );
        assert_eq!(
            max_list_page, 1000,
            "changing this changes what `db --limit` accepts"
        );
        assert!(list_page <= max_list_page);

        let SkillLimits {
            detailed_operations,
            listed_operations,
            line_chars,
            description_chars,
            enum_values,
        } = SKILL;
        // `api_skill::tests` renders one operation past each bound and at it.
        assert_eq!(
            detailed_operations, 25,
            "changing this changes which APIs --skill details"
        );
        assert_eq!(
            listed_operations, 200,
            "changing this changes how much of an API --skill indexes"
        );
        assert_eq!(line_chars, 160, "one line of a Skill");
        assert_eq!(description_chars, 800, "the description's paragraph");
        assert_eq!(enum_values, 5, "the values one parameter line names");
        assert!(detailed_operations < listed_operations);

        let McpHttpLimits {
            body_bytes,
            header_secs,
            body_secs,
        } = MCP_HTTP;
        // `mcp_http::tests` admits a Content-Length at the bound and refuses
        // one byte past it; `mcp_http_refuses_a_body_over_the_limit` sends one.
        assert_eq!(
            body_bytes,
            1024 * 1024,
            "changing this changes the largest tool call over HTTP"
        );
        assert_eq!(header_secs, 10, "a client sends its headers at once");
        assert_eq!(body_secs, 30, "a megabyte arrives well within this");

        let S3ReadLimits {
            preview_bytes,
            max_preview_bytes,
            gzip_transfer_bytes,
            hex_bytes,
            search_objects,
            object_bytes,
            total_bytes,
            matches,
            excerpt_chars,
        } = S3_READ;
        // `s3_command::tests` refuses one byte past `max_preview_bytes`;
        // `s3_read::tests` cut a gzip at `gzip_transfer_bytes`, a binary head
        // at `hex_bytes`, an excerpt at `excerpt_chars`, and a search at
        // `matches`, `object_bytes` and `total_bytes`.
        assert_eq!(preview_bytes, 65_536, "what --preview shows by default");
        assert_eq!(max_preview_bytes, 1_048_576, "what --bytes accepts");
        assert_eq!(gzip_transfer_bytes, 1_048_576, "what a gzip preview reads");
        assert_eq!(hex_bytes, 64, "the hex head of a binary object");
        assert_eq!(search_objects, 100, "what --search-content lists");
        assert_eq!(object_bytes, 8 * 1_048_576, "one searched object");
        assert_eq!(total_bytes, 50 * 1_048_576, "one content search");
        assert_eq!(matches, 1000, "matches one search reports");
        assert_eq!(excerpt_chars, 256, "one excerpt");
        assert!(preview_bytes <= max_preview_bytes && object_bytes <= total_bytes);

        let GraphQlLimits {
            response_depth,
            input_depth,
        } = GRAPHQL;
        // `graphql::tests` expands a return type to `response_depth` and an
        // input to `input_depth`, and finds the level below each bare.
        assert_eq!(
            response_depth, 2,
            "what --describe shows of a GraphQL result"
        );
        assert_eq!(input_depth, 4, "what --describe shows of a GraphQL input");

        let InputListLimits { listed } = INPUT_LIST;
        assert_eq!(listed, 100, "changing this changes what `data` reports");
        // One item past the bound is counted, not listed; at the bound
        // nothing is. Both `meta.inputs` and the Parquet footers cut here.
        let items: Vec<usize> = (0..=listed).collect();
        assert_eq!(INPUT_LIST.split(&items), (&items[..listed], 1));
        assert_eq!(INPUT_LIST.split(&items[..listed]), (&items[..listed], 0));
        assert_eq!(INPUT_LIST.split(&items[..0]), (&items[..0], 0));
    }
}
