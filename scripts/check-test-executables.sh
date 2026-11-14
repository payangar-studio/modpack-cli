#!/usr/bin/env bash
# Fails when test code makes a file executable itself instead of writing it
# with `write_executable` (tests/common/mod.rs). Writing a script and running
# it right away is flaky under parallel tests: a concurrent fork inherits the
# write descriptor, and the exec fails with ETXTBSY ("Text file busy").
#
# Pattern based. Test code is every file under tests/, every src file named
# tests.rs or under a tests/ directory, and in other src files what follows a
# `#[cfg(test)]` inline module. A false positive is fixed by using the helper.
set -euo pipefail
cd "$(dirname "$0")/.."

# A mode with the owner's execute bit, given to a file.
exec_mode='(from_mode|set_mode|[.]mode)[(] *0o[0-7]?[1357][0-7][0-7]([^0-7]|$)'

git ls-files -z -- 'tests/*.rs' 'src/*.rs' |
  grep -zvx 'tests/common/mod.rs' |
  xargs -0 awk -v re="$exec_mode" '
    FNR == 1 {
      intest = FILENAME ~ /^tests\// || FILENAME ~ /\/tests(\.rs|\/)/
      pending = 0
    }
    /^ *#\[cfg\((all\()?test[,)]/ { pending = 1; next }
    pending && /^ *(pub[^ ]* )?mod [A-Za-z_0-9]+ *\{/ { intest = 1 }
    !/^ *#\[/ { pending = 0 }
    intest && $0 ~ re {
      printf "%s:%d: a test makes a file executable itself; write it with write_executable (tests/common/mod.rs), which avoids ETXTBSY under parallel tests\n", FILENAME, FNR
      found = 1
    }
    END { exit found }
  '
