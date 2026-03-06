#!/usr/bin/env python3
"""Compare two or three Nominatim NDJSON files and report differences.

Usage:
    # Two-file comparison (expected vs actual):
    python3 compare-ndjson.py <expected.ndjson> <actual.ndjson> [options]

    # Three-file comparison (expected vs A vs B):
    python3 compare-ndjson.py <expected.ndjson> <a.ndjson> <b.ndjson> [options]

Examples:
    # Basic comparison
    compare-ndjson.py expected.ndjson actual.ndjson

    # Focus on address.street diffs
    compare-ndjson.py expected.ndjson actual.ndjson --field address --subfield street

    # Filter to point-accuracy entries only
    compare-ndjson.py expected.ndjson actual.ndjson --query 'c.get("extra",{}).get("accuracy")=="point"'

    # Three-way comparison (e.g. Kotlin vs Rust-cached vs Rust-uncached)
    compare-ndjson.py kotlin.ndjson rust-cached.ndjson rust-uncached.ndjson

    # Dump all address diffs as JSONL (pipe to jq, etc.)
    compare-ndjson.py expected.ndjson actual.ndjson --dump-diffs address

    # Check if address diffs correlate with centroid diffs
    compare-ndjson.py expected.ndjson actual.ndjson --correlate address centroid

    # Inspect a single entry across files
    compare-ndjson.py expected.ndjson actual.ndjson --inspect 50025416925

    # Show output ordering pattern (node/way/relation counts)
    compare-ndjson.py expected.ndjson actual.ndjson --order

    # Show value distribution for a subfield among differing entries
    compare-ndjson.py expected.ndjson actual.ndjson --histogram extra.accuracy
"""

import json
import sys
import argparse
from collections import Counter


# ---------------------------------------------------------------------------
# Loading
# ---------------------------------------------------------------------------

def load_entries(path):
    """Load NDJSON, returning (dict[place_id -> (full_doc, content)], order_list)."""
    entries = {}
    order = []
    with open(path) as f:
        for line in f:
            if not line.strip():
                continue
            d = json.loads(line)
            c = d.get("content", {})
            if isinstance(c, dict) and ("features" in c or "version" in c):
                continue  # header
            if isinstance(c, list):
                c = c[0] if c else {}
            if "place_id" in c:
                entries[c["place_id"]] = (d, c)
                order.append(c["place_id"])
    return entries, order


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def jdump(v):
    return json.dumps(v, sort_keys=True)


def entry_label(content):
    name = content.get("name")
    if isinstance(name, dict) and name.get("name"):
        return f' "{name["name"]}"'
    return ""


def get_nested(obj, dotpath):
    """Resolve a dot-separated path like 'extra.accuracy' against a dict."""
    for key in dotpath.split("."):
        if isinstance(obj, dict):
            obj = obj.get(key)
        else:
            return None
    return obj


def short_path(path):
    return path.rsplit("/", 1)[-1]


# ---------------------------------------------------------------------------
# Two-file comparison
# ---------------------------------------------------------------------------

def compare_two(expected, actual, max_examples=3, focus_field=None,
                focus_subfield=None, correlate=None, dump_diffs_field=None,
                histogram_path=None):
    common_ids = set(expected) & set(actual)

    # Compute diffs
    diff_count = 0
    field_diffs = Counter()
    field_examples = {}  # field -> [(pid, ev, av)]
    diff_pids = set()

    for pid in sorted(common_ids):
        ed, ec = expected[pid]
        ad, ac = actual[pid]
        if jdump(ed) == jdump(ad):
            continue
        diff_count += 1
        diff_pids.add(pid)
        for field in sorted(set(list(ec.keys()) + list(ac.keys()))):
            if jdump(ec.get(field)) != jdump(ac.get(field)):
                field_diffs[field] += 1
                if field not in field_examples:
                    field_examples[field] = []
                if len(field_examples[field]) < max_examples:
                    field_examples[field].append((pid, ec.get(field), ac.get(field)))

    missing_in_actual = set(expected) - set(actual)
    extra_in_actual = set(actual) - set(expected)
    if missing_in_actual:
        print(f"Missing in actual: {len(missing_in_actual)}")
        for pid in sorted(missing_in_actual)[:5]:
            print(f"  place_id={pid}{entry_label(expected[pid][1])}")
    if extra_in_actual:
        print(f"Extra in actual: {len(extra_in_actual)}")
        for pid in sorted(extra_in_actual)[:5]:
            print(f"  place_id={pid}{entry_label(actual[pid][1])}")

    identical = len(common_ids) - diff_count
    print(f"Identical: {identical}/{len(common_ids)}")
    print(f"Different: {diff_count}/{len(common_ids)}")
    print()

    if not field_diffs:
        print("No field-level differences found.")
        return

    print("Field diffs:")
    for field, count in field_diffs.most_common():
        print(f"  {field}: {count}")

    # --- Correlation ---
    if correlate:
        f1, f2 = correlate
        print(f"\n--- Correlation: {f1} vs {f2} ---")
        both = neither = f1_only = f2_only = 0
        for pid in common_ids:
            ec, ac = expected[pid][1], actual[pid][1]
            d1 = jdump(ec.get(f1)) != jdump(ac.get(f1))
            d2 = jdump(ec.get(f2)) != jdump(ac.get(f2))
            if d1 and d2: both += 1
            elif d1: f1_only += 1
            elif d2: f2_only += 1
            else: neither += 1
        print(f"  Both differ: {both}")
        print(f"  Only {f1}: {f1_only}")
        print(f"  Only {f2}: {f2_only}")
        print(f"  Neither: {neither}")

    # --- Histogram ---
    if histogram_path:
        print(f"\n--- Value histogram: {histogram_path} (among {len(diff_pids)} differing entries) ---")
        exp_vals = Counter()
        act_vals = Counter()
        for pid in diff_pids:
            ev = get_nested(expected[pid][1], histogram_path)
            av = get_nested(actual[pid][1], histogram_path)
            exp_vals[str(ev)] += 1
            act_vals[str(av)] += 1
        all_keys = sorted(set(exp_vals) | set(act_vals))
        print(f"  {'value':<30} expected  actual")
        for k in all_keys:
            print(f"  {k:<30} {exp_vals[k]:>8}  {act_vals[k]:>6}")

    # --- Dump diffs ---
    if dump_diffs_field:
        f = dump_diffs_field
        for pid in sorted(diff_pids):
            ec, ac = expected[pid][1], actual[pid][1]
            ev, av = ec.get(f), ac.get(f)
            if jdump(ev) != jdump(av):
                row = {"place_id": pid, "expected": ev, "actual": av,
                       "centroid": ec.get("centroid")}
                name = ec.get("name")
                if isinstance(name, dict):
                    row["name"] = name.get("name")
                print(json.dumps(row))
        return

    # --- Detailed field analysis ---
    fields_to_show = [focus_field] if focus_field else [f for f, _ in field_diffs.most_common()]
    for field in fields_to_show:
        if field not in field_examples:
            continue
        print(f"\n--- {field} ({field_diffs[field]} diffs) ---")
        _analyze_field(field, field_examples[field], diff_pids, expected, actual,
                       focus_subfield)

        for pid, ev, av in field_examples[field]:
            label = entry_label(expected[pid][1])
            print(f"  pid={pid}{label}:")
            print(f"    expected: {json.dumps(ev)[:200]}")
            print(f"    actual:   {json.dumps(av)[:200]}")


def _analyze_field(field, examples, diff_pids, expected, actual, focus_subfield=None):
    sample = examples[0][1] if examples else None

    # Dict fields: subfield breakdown
    if isinstance(sample, dict):
        subfield_counts = Counter()
        for pid in diff_pids:
            ev = expected[pid][1].get(field, {})
            av = actual[pid][1].get(field, {})
            if not isinstance(ev, dict) or not isinstance(av, dict):
                continue
            for sf in sorted(set(list(ev.keys()) + list(av.keys()))):
                if focus_subfield and sf != focus_subfield:
                    continue
                if jdump(ev.get(sf)) != jdump(av.get(sf)):
                    subfield_counts[sf] += 1
        if subfield_counts:
            print(f"  Subfield breakdown:")
            for sf, cnt in subfield_counts.most_common():
                exp_only = act_only = both_diff = 0
                for pid in diff_pids:
                    ev = expected[pid][1].get(field, {})
                    av = actual[pid][1].get(field, {})
                    if not isinstance(ev, dict) or not isinstance(av, dict):
                        continue
                    esf, asf = ev.get(sf), av.get(sf)
                    if jdump(esf) == jdump(asf):
                        continue
                    if esf is not None and asf is not None:
                        both_diff += 1
                    elif esf is not None:
                        exp_only += 1
                    else:
                        act_only += 1
                parts = []
                if both_diff: parts.append(f"different={both_diff}")
                if exp_only: parts.append(f"expected_only={exp_only}")
                if act_only: parts.append(f"actual_only={act_only}")
                print(f"    {sf}: {cnt}  ({', '.join(parts)})")

    # List fields: order vs content
    elif isinstance(sample, list):
        order_only = content_diff = 0
        for pid in diff_pids:
            ev = expected[pid][1].get(field, [])
            av = actual[pid][1].get(field, [])
            if not isinstance(ev, list) or not isinstance(av, list):
                continue
            if jdump(ev) == jdump(av):
                continue
            if sorted(ev) == sorted(av):
                order_only += 1
            else:
                content_diff += 1
        if order_only or content_diff:
            print(f"  List breakdown: order_only={order_only}, content_different={content_diff}")


# ---------------------------------------------------------------------------
# Three-file comparison
# ---------------------------------------------------------------------------

def compare_three(expected, a_entries, b_entries, a_label, b_label,
                  focus_field=None, focus_subfield=None):
    common = set(expected) & set(a_entries) & set(b_entries)
    print(f"Common entries across all three: {len(common)}")
    print()

    def get_val(entries, pid, field, subfield=None):
        v = entries[pid][1].get(field)
        if subfield and isinstance(v, dict):
            v = v.get(subfield)
        return v

    fields = set()
    for pid in common:
        fields.update(expected[pid][1].keys())

    target_fields = [focus_field] if focus_field else sorted(fields)

    for field in target_fields:
        sf = focus_subfield
        label = f"{field}.{sf}" if sf else field
        both_match = both_diff_same = both_diff_different = a_only = b_only = 0
        for pid in common:
            ev = get_val(expected, pid, field, sf)
            av = get_val(a_entries, pid, field, sf)
            bv = get_val(b_entries, pid, field, sf)
            ej = jdump(ev)
            a_eq = (jdump(av) == ej)
            b_eq = (jdump(bv) == ej)
            if a_eq and b_eq:
                both_match += 1
            elif a_eq:
                b_only += 1
            elif b_eq:
                a_only += 1
            elif jdump(av) == jdump(bv):
                both_diff_same += 1
            else:
                both_diff_different += 1

        total_diffs = a_only + b_only + both_diff_same + both_diff_different
        if total_diffs == 0:
            continue
        print(f"--- {label} ---")
        print(f"  Both match expected:        {both_match}")
        print(f"  Only {a_label} differs:  {a_only}")
        print(f"  Only {b_label} differs:  {b_only}")
        print(f"  Both differ (same value):   {both_diff_same}")
        print(f"  Both differ (differently):  {both_diff_different}")
        print()


# ---------------------------------------------------------------------------
# Inspect single entry
# ---------------------------------------------------------------------------

def inspect_entry(all_loaded, place_id):
    """Show a single entry side-by-side across all files."""
    for entries, order, path in all_loaded:
        label = short_path(path)
        if place_id in entries:
            _, c = entries[place_id]
            print(f"=== {label} (place_id={place_id}){entry_label(c)} ===")
            print(json.dumps(c, indent=2, ensure_ascii=False))
        else:
            print(f"=== {label}: NOT FOUND ===")
        print()


# ---------------------------------------------------------------------------
# Order analysis
# ---------------------------------------------------------------------------

def analyze_order(all_loaded):
    """Compare entry ordering and show entity-type pattern."""
    print("--- Entry ordering ---")
    base_entries, base_order, base_path = all_loaded[0]

    for entries, order, path in all_loaded:
        label = short_path(path)
        # Show entity-type pattern (point/polygon/relation)
        pattern = Counter()
        for pid in order:
            acc = entries[pid][1].get("extra", {}).get("accuracy", "?")
            pattern[acc] += 1
        parts = ", ".join(f"{k}: {v}" for k, v in pattern.most_common())
        print(f"  {label}: {parts}")

    # Compare ordering
    for entries, order, path in all_loaded[1:]:
        label = short_path(path)
        if order == base_order:
            print(f"  {label}: same order as {short_path(base_path)}")
        else:
            diff_pos = sum(1 for a, b in zip(base_order, order) if a != b)
            total = min(len(base_order), len(order))
            print(f"  {label}: {diff_pos}/{total} entries in different position")
            for i, (a, b) in enumerate(zip(base_order, order)):
                if a != b:
                    print(f"    First diff at index {i}: expected={a}, actual={b}")
                    break
    print()


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Compare Nominatim NDJSON files",
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("files", nargs="+", help="NDJSON files: expected actual [variant_b]")
    parser.add_argument("--examples", type=int, default=3,
                        help="Max examples per field (default: 3)")
    parser.add_argument("--field", help="Focus on a specific field")
    parser.add_argument("--subfield",
                        help="Focus on a subfield within --field (e.g. street within address)")
    parser.add_argument("--query",
                        help="Filter entries: Python expr over content dict `c`")
    parser.add_argument("--correlate", nargs=2, metavar=("F1", "F2"),
                        help="Show correlation between diffs in two fields")
    parser.add_argument("--dump-diffs", metavar="FIELD",
                        help="Dump all differing values for FIELD as JSONL")
    parser.add_argument("--order", action="store_true",
                        help="Compare entry ordering and show entity-type pattern")
    parser.add_argument("--inspect", type=int, metavar="PLACE_ID",
                        help="Show a single entry across all files")
    parser.add_argument("--histogram", metavar="DOTPATH",
                        help="Show value distribution for a dotpath (e.g. extra.accuracy) "
                             "among differing entries")
    args = parser.parse_args()

    if len(args.files) < 2 or len(args.files) > 3:
        parser.error("Provide 2 or 3 NDJSON files")

    all_loaded = []
    for path in args.files:
        entries, order = load_entries(path)
        all_loaded.append((entries, order, path))

    # Query filter
    if args.query:
        pred = lambda c: eval(args.query)  # noqa: S307
        filtered = []
        for entries, order, path in all_loaded:
            fe = {pid: (d, c) for pid, (d, c) in entries.items() if pred(c)}
            fo = [pid for pid in order if pid in fe]
            filtered.append((fe, fo, path))
        all_loaded = filtered

    for entries, order, path in all_loaded:
        print(f"{short_path(path)}: {len(entries)} entries")
    print()

    # Inspect single entry
    if args.inspect is not None:
        inspect_entry(all_loaded, args.inspect)
        return

    # Order analysis
    if args.order:
        analyze_order(all_loaded)

    expected = all_loaded[0][0]

    if len(all_loaded) == 2:
        actual = all_loaded[1][0]
        compare_two(expected, actual, args.examples, args.field,
                    args.subfield, args.correlate, args.dump_diffs,
                    args.histogram)
    else:
        a_entries = all_loaded[1][0]
        b_entries = all_loaded[2][0]
        a_label = short_path(args.files[1])
        b_label = short_path(args.files[2])

        print(f"=== Expected vs {a_label} ===")
        compare_two(expected, a_entries, args.examples, args.field,
                    args.subfield, args.correlate, args.dump_diffs,
                    args.histogram)

        print(f"\n=== Expected vs {b_label} ===")
        compare_two(expected, b_entries, args.examples, args.field,
                    args.subfield, args.correlate, args.dump_diffs,
                    args.histogram)

        print(f"\n=== Three-way: expected vs {a_label} vs {b_label} ===")
        compare_three(expected, a_entries, b_entries, a_label, b_label,
                      args.field, args.subfield)


if __name__ == "__main__":
    main()
