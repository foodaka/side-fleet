#!/bin/sh
# Fake opencode CLI for tests. Prints `opencode run --format json` lines
# (shapes from opencode 1.19). The task arrives on stdin; the final text
# echoes it, the session it resumed and OPENCODE_PERMISSION. A task
# containing "fail" ends with a session error and exit 1.
case "$1" in
  --version) printf '%s\n' "9.9.9"; exit 0;;
  models) printf '%s\n' "openrouter/moonshotai/kimi-k2" "openrouter/z-ai/glm-4.6"; exit 0;;
esac
session="ses_fakeNew"
while [ $# -gt 0 ]; do
  case "$1" in
    --session) session="$2"; shift 2;;
    *) shift;;
  esac
done
task=$(cat)
esc() { printf '%s' "$1" | sed 's/"/\\"/g'; }
s="\"sessionID\":\"$session\""
printf '%s\n' "{\"type\":\"step_start\",\"timestamp\":1,$s,\"part\":{\"type\":\"step-start\"}}"
sleep 0.1
printf '%s\n' "{\"type\":\"tool_use\",\"timestamp\":2,$s,\"part\":{\"type\":\"tool\",\"callID\":\"c1\",\"tool\":\"bash\",\"state\":{\"status\":\"completed\",\"input\":{\"command\":\"git status --short\"},\"output\":\"?? notes.txt\"}}}"
printf '%s\n' "{\"type\":\"step_finish\",\"timestamp\":3,$s,\"part\":{\"type\":\"step-finish\",\"reason\":\"tool-calls\",\"cost\":0.001,\"tokens\":{\"input\":100,\"output\":10}}}"
case "$task" in
  *fail*)
    printf '%s\n' "{\"type\":\"error\",\"timestamp\":4,$s,\"error\":{\"name\":\"APIError\",\"data\":{\"message\":\"simulated provider error\"}}}"
    exit 1;;
esac
printf '%s\n' "{\"type\":\"text\",\"timestamp\":5,$s,\"part\":{\"type\":\"text\",\"text\":\"task=$(esc "$task") perm=$(esc "$OPENCODE_PERMISSION")\"}}"
printf '%s\n' "{\"type\":\"step_finish\",\"timestamp\":6,$s,\"part\":{\"type\":\"step-finish\",\"reason\":\"stop\",\"cost\":0.002,\"tokens\":{\"input\":200,\"output\":20}}}"
exit 0
