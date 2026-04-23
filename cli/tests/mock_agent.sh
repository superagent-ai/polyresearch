#!/bin/bash
# Mock agent for scenario tests.
# Controlled by MOCK_AGENT_RESULT: improved, no_improvement, crashed, fail,
#   fail_once, hang, noop, no_improvement_with_changes
RESULT_DIR="$PWD/.polyresearch"
mkdir -p "$RESULT_DIR"

case "${MOCK_AGENT_RESULT:-improved}" in
    improved)
        cat > "$RESULT_DIR/result.json" <<'RESULT'
{"metric":0.95,"baseline":0.90,"observation":"improved","summary":"mock improvement"}
RESULT
        mkdir -p src
        echo "// mock change $(date +%s)" >> src/mock.js
        ;;
    no_improvement)
        cat > "$RESULT_DIR/result.json" <<'RESULT'
{"metric":0.89,"baseline":0.90,"observation":"no_improvement","summary":"mock no change"}
RESULT
        ;;
    no_improvement_with_changes)
        cat > "$RESULT_DIR/result.json" <<'RESULT'
{"metric":0.89,"baseline":0.90,"observation":"no_improvement","summary":"mock no change but kept edits"}
RESULT
        mkdir -p src
        echo "// mock change $(date +%s)" >> src/mock.js
        ;;
    crashed)
        cat > "$RESULT_DIR/result.json" <<'RESULT'
{"metric":0.0,"baseline":0.90,"observation":"crashed","summary":"mock crash"}
RESULT
        ;;
    fail)
        exit 1
        ;;
    fail_once)
        COUNTER_FILE="${RESULT_DIR}/.mock-run-count"
        COUNT=$(cat "$COUNTER_FILE" 2>/dev/null || echo 0)
        COUNT=$((COUNT + 1))
        echo "$COUNT" > "$COUNTER_FILE"
        # Threshold is 2 because the preflight smoke test consumes one invocation.
        if [ "$COUNT" -le 2 ]; then
            exit 1
        fi
        exit 0
        ;;
    noop)
        exit 0
        ;;
    hang)
        sleep 999999
        ;;
esac
