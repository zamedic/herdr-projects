#!/bin/sh
# The fork's one check, `npm run check` (the work board runs `npm run check -- --all`): the same steps as the Check
# workflow (.github/workflows/check.yml) that are safe on a person's machine. Arguments (--all, --since <commit>) are
# taken and ignored: the whole check runs every time, in seconds. herdr-task's command test still runs only on CI.
set -e
cd "$(dirname "$0")/.."
echo "check: shared lib copies"
python3 bin/sync-shared --check
echo "check: JavaScript parses"
fail=0
for f in $(find mods lib -name '*.js'); do
  node --input-type=module --check < "$f" || { echo "check: $f doesn't parse"; fail=1; }
done
[ "$fail" = 0 ]
if command -v bun >/dev/null 2>&1; then
  echo "check: TypeScript helpers build"
  for f in $(find mods -name '*.ts'); do bun build --no-bundle --target bun "$f" > /dev/null; done
else
  echo "check: no bun here, the TypeScript helpers are left to CI"
fi
echo "check: JSON parses"
for f in keys.json layouts/*.json lib/addons.json $(find mods -name '*.json'); do python3 -m json.tool "$f" > /dev/null; done
echo "check: shell scripts parse"
for f in install.sh guard/herdr-app scripts/check.sh $(find mods -name '*.sh'); do
  case "$(head -1 "$f")" in *bash*) bash -n "$f" ;; *) sh -n "$f" ;; esac
done
echo "check: tests (repo rules, installer, herdr-task, Azure DevOps)"
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s test
