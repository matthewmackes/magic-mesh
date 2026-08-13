#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_root="${MCNF_SQLITE_AUTHORITY_SOURCE_ROOT:-$repo_root/crates/mesh/mackesd/src}"
baseline="${MCNF_SQLITE_AUTHORITY_BASELINE:-$repo_root/docs/platform/mackesd-sqlite-direct-write-baseline.tsv}"

scan() {
  local destination="$1" file
  while IFS= read -r file; do
    [[ "$file" == */store/* ]] && continue
    awk -v file="$file" '
      function brace_delta(line, copy, opens, closes) {
        copy=line; opens=gsub(/\{/, "", copy)
        copy=line; closes=gsub(/\}/, "", copy)
        return opens-closes
      }
      skip_test {
        test_depth += brace_delta($0)
        if (test_depth <= 0) { skip_test=0; test_depth=0 }
        next
      }
      pending_test {
        delta=brace_delta($0)
        if (index($0, "{")) {
          pending_test=0
          if (delta > 0) { skip_test=1; test_depth=delta }
          next
        }
        if ($0 ~ /[;,][[:space:]]*$/) { pending_test=0 }
        next
      }
      /^[[:space:]]*#\[cfg\(test\)\]/ { pending_test=1; next }
      /\.(execute|execute_batch|transaction|unchecked_transaction)[[:space:]]*\(|store::with_transaction[[:space:]]*\(/ {
        print file ":" FNR ":" $0
      }
    ' "$file"
  done < <(rg --files --glob '*.rs' "$source_root") \
    | cut -d: -f1 \
    | sed "s#^$repo_root/##" \
    | sort \
    | uniq -c \
    | awk '{print $2 "\t" $1}' \
    | sort >"$destination"
}

if [[ "${1:-}" == "--self-test" ]]; then
  fixture="$(mktemp -d)"
  trap 'rm -rf -- "$fixture"' EXIT
  mkdir -p "$fixture/src/store"
  printf '%s\n' \
    '#[cfg(test)]' \
    'fn fixture_write(c: &C) { c.execute_batch("TEST"); }' \
    'fn existing(c: &C) { c.execute("DELETE", []); }' >"$fixture/src/live.rs"
  printf '%s\n' \
    '#[cfg(test)]' \
    'mod tests {' \
    '    fn fixture_write(c: &C) { c.execute("TEST", []); }' \
    '}' >"$fixture/src/test_module.rs"
  printf '%s\n' 'fn owner(c: &C) { c.execute("INSERT", []); }' >"$fixture/src/store/writer.rs"
  printf '%s\t%s\n' "$fixture/src/live.rs" 1 >"$fixture/baseline.tsv"
  MCNF_SQLITE_AUTHORITY_SOURCE_ROOT="$fixture/src" \
    MCNF_SQLITE_AUTHORITY_BASELINE="$fixture/baseline.tsv" "$0"
  printf '%s\n' 'fn drift(c: &C) { c.transaction(); }' >>"$fixture/src/live.rs"
  if MCNF_SQLITE_AUTHORITY_SOURCE_ROOT="$fixture/src" \
    MCNF_SQLITE_AUTHORITY_BASELINE="$fixture/baseline.tsv" "$0" >/dev/null 2>&1; then
    echo "sqlite-authority self-test: new direct write escaped the negative gate" >&2
    exit 1
  fi
  echo "sqlite-authority self-test: PASS"
  exit 0
fi

actual="$(mktemp)"
trap 'rm -f -- "$actual"' EXIT
scan "$actual"
if ! diff -u "$baseline" "$actual"; then
  echo "mackesd SQLite authority: direct write set changed; migrate new writes through store::writer or update the reviewed residual inventory" >&2
  exit 1
fi

if rg -q 'ExecuteSql|RawSql|SqlStatement|sql:[[:space:]]*String' \
  "$repo_root/crates/mesh/mackesd/src/store/writer.rs"; then
  echo "mackesd SQLite authority: writer protocol appears to admit SQL-shaped operations" >&2
  exit 1
fi

echo "mackesd SQLite authority: PASS ($(awk '{n += $2} END {print n + 0}' "$actual") reviewed residual syntax sites)"
