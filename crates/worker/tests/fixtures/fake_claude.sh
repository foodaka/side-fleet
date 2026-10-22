#!/bin/sh
# Fake Claude Code CLI for tests. Prints stream-json lines slowly.
# Understands: --version, -p <task> (task containing "slow" sleeps a long time
# before the result, unless it was moved from another machine; "fail" exits
# non-zero; "error-result" reports is_error; "write" creates failover.txt during
# the Bash tool call when it is not there yet; "edit" changes README.md and adds
# notes.txt; "rename" moves notes.txt).
if [ "$1" = "--version" ]; then printf '%s\n' "9.9.9 (Claude Code fake)"; exit 0; fi
task=""
model="unknown"
while [ $# -gt 0 ]; do
  case "$1" in
    -p) task="$2"; shift 2;;
    --model) model="$2"; shift 2;;
    *) shift;;
  esac
done
# Start a child so the test can verify the whole process group is killed.
sleep 300 &
child=$!
printf '%s\n' "{\"type\":\"system\",\"subtype\":\"hook_started\",\"hook_name\":\"SessionStart\"}"
printf '%s\n' "{\"type\":\"system\",\"subtype\":\"init\",\"model\":\"$model\",\"session_id\":\"fake-session\",\"tools\":[\"Bash\",\"Read\"],\"cwd\":\"$(pwd)\"}"
sleep 0.2
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Looking at the repository first."}]}}'
sleep 0.2
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"export GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz123456 && git status --short"}}]}}'
sleep 0.2
case "$task" in
  *write*) [ -f failover.txt ] || printf 'half done\n' > failover.txt;;
esac
printf '%s\n' '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":"## main\n?? notes.txt\nAPI_KEY=supersecretvalue99","is_error":false}]}}'
sleep 0.2
printf '%s\n' "fake stderr line with token=abcdefghijklmnop" >&2
case "$task" in
  *"Sidefleet moved this task"*) ;;
  *slow*) sleep 60;;
esac
case "$task" in
  *edit*) printf 'demo\nedited by the agent\n' > README.md; printf 'one\ntwo\n' > notes.txt;;
  *rename*) git mv -f notes.txt docs-notes.txt 2>/dev/null || mv notes.txt docs-notes.txt;;
esac
case "$task" in
  *fail*) printf '%s\n' "fatal: simulated crash" >&2; kill $child 2>/dev/null; exit 3;;
  *error-result*) printf '%s\n' '{"type":"result","subtype":"error_max_turns","is_error":true,"duration_ms":900,"num_turns":1,"session_id":"fake-session"}'; kill $child 2>/dev/null; exit 0;;
esac
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"The working tree has one untracked file."}]}}'
sleep 0.2
printf '%s\n' '{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}'
printf '%s\n' "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"Done: one untracked file (notes.txt). Model was $model.\",\"duration_ms\":1200,\"num_turns\":2,\"total_cost_usd\":0.0042,\"session_id\":\"fake-session\"}"
kill $child 2>/dev/null
exit 0
