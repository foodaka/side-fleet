#!/bin/sh
# Fake Codex CLI for tests. Prints `codex exec --json` lines (shapes from
# codex-cli 0.162). The final message echoes the task and the session it
# resumed, so tests can check what reached the agent. A task containing
# "fail" ends with turn.failed and exit 1.
case "$1" in
  --version) printf '%s\n' "codex-cli 9.9.9"; exit 0;;
  login) printf '%s\n' "Logged in using ChatGPT" >&2; exit 0;;
  debug) printf '%s\n' '{"models":[{"slug":"gpt-fake","visibility":"list","priority":1}]}'; exit 0;;
esac
shift # exec
thread="fake-thread"
resumed=""
if [ "$1" = "resume" ]; then shift; resumed="yes"; fi
while [ $# -gt 0 ]; do
  case "$1" in
    --) shift; break;;
    -m|-c) shift 2;;
    *) shift;;
  esac
done
if [ -n "$resumed" ]; then thread="$1"; shift; fi
task="$1"
printf '%s\n' "{\"type\":\"thread.started\",\"thread_id\":\"$thread\"}"
printf '%s\n' '{"type":"turn.started"}'
sleep 0.1
printf '%s\n' '{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"thinking"}}'
printf '%s\n' '{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc \"git status --short\"","aggregated_output":"","exit_code":null,"status":"in_progress"}}'
sleep 0.1
printf '%s\n' '{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc \"git status --short\"","aggregated_output":"?? notes.txt\nAPI_KEY=supersecretvalue99\n","exit_code":0,"status":"completed"}}'
case "$task" in
  *fail*)
    printf '%s\n' '{"type":"turn.failed","error":{"message":"{\"type\":\"error\",\"status\":400,\"error\":{\"message\":\"simulated model error\"}}"}}'
    exit 1;;
esac
# JSON-escape the task (a fork's task is a multi-line transcript).
esc=$(printf '%s' "$task" | awk 'BEGIN{ORS=""} {gsub(/\\/,"\\\\"); gsub(/"/,"\\\""); gsub(/\t/,"\\t"); if (NR>1) print "\\n"; print}')
printf '%s\n' "{\"type\":\"item.completed\",\"item\":{\"id\":\"item_2\",\"type\":\"agent_message\",\"text\":\"task=$esc\"}}"
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":10}}'
exit 0
