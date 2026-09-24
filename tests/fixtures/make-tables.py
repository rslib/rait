#!/usr/bin/env python3
"""Generate deterministic table fixtures (csv, tsv, xlsx, parquet, edge csv).

Run via tests/fixtures/make-tables.sh, which invokes this script with uv so
openpyxl and pyarrow do not need to be installed globally.
"""

import csv
import datetime
import io
import os
import re
import zipfile

import pyarrow as pa
import pyarrow.csv as pa_csv
import pyarrow.parquet as pq
from openpyxl import Workbook
from openpyxl.cell.cell import Cell

HERE = os.path.dirname(os.path.abspath(__file__))
FIXED_ZIP_TIME = (2026, 1, 1, 0, 0, 0)


def rewrite_xlsx_deterministically(path, fixed_modified_iso):
    """Force docProps/core.xml `modified` and all zip entry timestamps to be
    fixed, undoing openpyxl's writer, which always stamps `modified` with
    datetime.now() at save time (openpyxl.writer.excel.save_workbook).
    """
    with open(path, "rb") as f:
        src = zipfile.ZipFile(io.BytesIO(f.read()))
        entries = {name: src.read(name) for name in src.namelist()}

    core = entries["docProps/core.xml"].decode("utf-8")
    core = re.sub(
        r"(<dcterms:modified[^>]*>)[^<]*(</dcterms:modified>)",
        rf"\g<1>{fixed_modified_iso}\g<2>",
        core,
    )
    entries["docProps/core.xml"] = core.encode("utf-8")

    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as out:
        for name in sorted(entries):
            info = zipfile.ZipInfo(name, date_time=FIXED_ZIP_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            out.writestr(info, entries[name])

    with open(path, "wb") as f:
        f.write(buf.getvalue())


def row(r):
    region = ["EU", "US", "APAC"][(r - 1) % 3]
    quarter = "Q" + str((r - 1) % 4 + 1)
    revenue = round(r * 10.5, 2)
    units = r * 3
    active = r % 2 == 0
    zip_ = "0" + str(1000 + r)
    day = datetime.date(2026, 1, r)
    note = None if r % 5 == 0 else f"row {r}"
    return {
        "region": region,
        "quarter": quarter,
        "revenue": revenue,
        "units": units,
        "active": active,
        "zip": zip_,
        "day": day,
        "note": note,
    }


ROWS = [row(r) for r in range(1, 26)]
COLUMNS = ["region", "quarter", "revenue", "units", "active", "zip", "day", "note"]


def write_delimited(path, delimiter):
    with open(path, "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f, delimiter=delimiter, lineterminator="\n")
        w.writerow(COLUMNS)
        for rec in ROWS:
            w.writerow(
                [
                    rec["region"],
                    rec["quarter"],
                    f"{rec['revenue']}",
                    rec["units"],
                    "true" if rec["active"] else "false",
                    rec["zip"],
                    rec["day"].isoformat(),
                    "" if rec["note"] is None else rec["note"],
                ]
            )


def write_csv():
    write_delimited(os.path.join(HERE, "sample.csv"), ",")


def write_tsv():
    write_delimited(os.path.join(HERE, "sample.tsv"), "\t")


def write_xlsx():
    wb = Workbook()
    fixed_dt = datetime.datetime(2026, 1, 1)
    wb.properties.created = fixed_dt
    wb.properties.modified = fixed_dt

    ws = wb.active
    ws.title = "Sales"
    ws.append(COLUMNS)
    for rec in ROWS:
        ws.append(
            [
                rec["region"],
                rec["quarter"],
                float(rec["revenue"]),
                int(rec["units"]),
                bool(rec["active"]),
                rec["zip"],
                rec["day"],
                rec["note"],
            ]
        )

    # Force zip column to be stored as text and day column as a real date.
    zip_col = COLUMNS.index("zip") + 1
    day_col = COLUMNS.index("day") + 1
    for r in range(2, ws.max_row + 1):
        zip_cell: Cell = ws.cell(row=r, column=zip_col)
        zip_cell.data_type = "s"
        zip_cell.number_format = "@"

        day_cell: Cell = ws.cell(row=r, column=day_col)
        day_cell.number_format = "yyyy-mm-dd"

    notes = wb.create_sheet("Notes")
    notes.append(["key", "value"])
    notes.append(["owner", "ray"])
    notes.append(["version", 3])

    xlsx_path = os.path.join(HERE, "sample.xlsx")
    wb.save(xlsx_path)
    rewrite_xlsx_deterministically(xlsx_path, "2026-01-01T00:00:00Z")


def build_arrow_table():
    schema = pa.schema(
        [
            ("region", pa.string()),
            ("quarter", pa.string()),
            ("revenue", pa.float64()),
            ("units", pa.int64()),
            ("active", pa.bool_()),
            ("zip", pa.string()),
            ("day", pa.date32()),
            ("note", pa.string()),
        ]
    )
    columns = {name: [] for name in COLUMNS}
    for rec in ROWS:
        for name in COLUMNS:
            columns[name].append(rec[name])
    arrays = [pa.array(columns[name], type=schema.field(name).type) for name in COLUMNS]
    return pa.Table.from_arrays(arrays, schema=schema)


def write_parquet():
    table = build_arrow_table()
    pq.write_table(
        table,
        os.path.join(HERE, "sample.parquet"),
        compression="snappy",
    )
    pq.write_table(
        table,
        os.path.join(HERE, "sample-zstd.parquet"),
        compression="zstd",
    )


def write_edge_csv():
    path = os.path.join(HERE, "edge.csv")
    parts = []
    parts.append(b'\xef\xbb\xbf')  # UTF-8 BOM
    parts.append(b'name,name,,"amount, usd"\r\n')
    parts.append(b'alice,"Smith, Jr.",x,10\r\n')
    parts.append(b'bob,"line one\nline two",y,20\r\n')
    parts.append(b'carol,"She said ""hi""",z,30\r\n')
    parts.append(b'dave,short\r\n')
    with open(path, "wb") as f:
        for p in parts:
            f.write(p)


def verify():
    print("=== verify ===")

    def show_csv(path, delimiter=","):
        with open(path, "r", newline="", encoding="utf-8") as f:
            r = list(csv.reader(f, delimiter=delimiter))
        print(f"{path}: {len(r) - 1} data rows")
        for line in r[:3]:
            print("  ", line)

    show_csv(os.path.join(HERE, "sample.csv"), ",")
    show_csv(os.path.join(HERE, "sample.tsv"), "\t")

    from openpyxl import load_workbook

    wb = load_workbook(os.path.join(HERE, "sample.xlsx"))
    ws = wb["Sales"]
    print(f"sample.xlsx Sales: {ws.max_row - 1} data rows, sheets={wb.sheetnames}")
    for r in range(1, 4):
        print("  ", [c.value for c in ws[r]])
    zip_cell = ws.cell(row=2, column=COLUMNS.index("zip") + 1)
    day_cell = ws.cell(row=2, column=COLUMNS.index("day") + 1)
    print(
        f"  zip cell row2: value={zip_cell.value!r} data_type={zip_cell.data_type!r} "
        f"number_format={zip_cell.number_format!r}"
    )
    print(
        f"  day cell row2: value={day_cell.value!r} data_type={day_cell.data_type!r} "
        f"number_format={day_cell.number_format!r}"
    )

    for name in ("sample.parquet", "sample-zstd.parquet"):
        p = os.path.join(HERE, name)
        table = pq.read_table(p)
        print(f"{name}: {table.num_rows} rows")
        print("  ", table.slice(0, 2).to_pylist())
        meta = pq.read_metadata(p)
        col0 = meta.row_group(0).column(0)
        print(
            f"  metadata num_rows={meta.num_rows} col0_compression={col0.compression}"
        )

    edge_path = os.path.join(HERE, "edge.csv")
    with open(edge_path, "rb") as f:
        raw = f.read()
    has_bom = raw[:3] == b"\xef\xbb\xbf"
    print(f"edge.csv: {len(raw)} bytes, starts_with_bom={has_bom}")
    # Note: edge.csv has an intentional ragged last row, which pyarrow's
    # strict CSV parser rejects by default. Verify with the stdlib csv
    # module instead, which tolerates ragged rows.
    with open(edge_path, "r", newline="", encoding="utf-8-sig") as f:
        edge_rows = list(csv.reader(f))
    print(f"  stdlib csv read {len(edge_rows) - 1} data rows, header={edge_rows[0]}")
    for line in edge_rows[:3]:
        print("  ", line)


def main():
    write_csv()
    write_tsv()
    write_xlsx()
    write_parquet()
    write_edge_csv()
    verify()


if __name__ == "__main__":
    main()
