#!/bin/bash
# Mock agent for scenario tests.
# Controlled by MOCK_AGENT_RESULT: improved, no_improvement, crashed, fail, hang,
#   no_improvement_with_changes
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
    hang)
        sleep 999999
        ;;
esac
