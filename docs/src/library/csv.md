# cdt.csv

RFC 4180 CSV parser and serializer with custom delimiter support and header-mapped records.

## Import

```conduit
import * as csv from "cdt.csv"
// or
import { parse, parse_with, parse_records, stringify, stringify_with, CsvError } from "cdt.csv"
```

## Types

### `CsvError`

```conduit
struct CsvError {
    kind: String     // "InvalidCsv", "InvalidDelimiter", or "ColumnMismatch"
    line: Int        // 1-based line number (0 for delimiter errors)
    message: String
}
```

## Functions

| Function | Signature | Description |
| :--- | :--- | :--- |
| `parse` | `csv.parse(text: String) -> [[String]] \| CsvError` | Parses comma-separated RFC 4180 CSV text into rows. |
| `parse_with` | `csv.parse_with(text: String, delimiter: String) -> [[String]] \| CsvError` | Parses CSV text using a single-character `delimiter` (e.g. `";"`, `"\t"`). |
| `parse_records` | `csv.parse_records(text: String) -> [Map<String, String>] \| CsvError` | Uses the first row as column headers and returns each subsequent row as a map. |
| `stringify` | `csv.stringify(rows: [[String]]) -> String` | Serializes rows into RFC 4180 CSV text, quoting fields when needed. |
| `stringify_with` | `csv.stringify_with(rows: [[String]], delimiter: String) -> String \| CsvError` | Serializes rows using a single-character `delimiter`. |

## Example

```conduit
import * as csv from "cdt.csv"

func main() -> Void {
    raw := "name,role\n\"Ada, Lovelace\",engineer\n"
    switch csv.parse_records(raw) {
        case [Map<String, String>](records):
            print("Loaded {records.len()} record(s)")
        case csv.CsvError(err):
            print("CSV error [{err.kind}] on line {err.line}: {err.message}")
    }
}
```
