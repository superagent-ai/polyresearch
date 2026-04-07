# Vendored Sources

This example vendors a small subset of files from
`https://github.com/SakanaAI/drq`, which is licensed under Apache 2.0.

Included upstream artifacts:

- `.polyresearch/corewar/core.py`
- `.polyresearch/corewar/mars.py`
- `.polyresearch/corewar/redcode.py`
- `.polyresearch/opponents/imp.red`
- `.polyresearch/opponents/dwarf.red`
- `.polyresearch/opponents/mice.red`
- `.polyresearch/opponents/nonzeroscanner.red`
- `.polyresearch/opponents/dwarfmice.red`
- `.polyresearch/opponents/impgate.red`
- `.polyresearch/opponents/rato.red`
- `.polyresearch/opponents/stone.red`

Local modifications:

- Added `.polyresearch/corewar/__init__.py` for cleaner package imports.
- Hardened `.polyresearch/corewar/redcode.py` so Redcode expressions are parsed with a
  restricted arithmetic evaluator instead of raw Python `eval()`. This keeps
  `warrior.red` inside the intended trust boundary.
- Copied the upstream Apache 2.0 text to `.polyresearch/LICENSE.upstream`.

The DRQ repository notes that its Core War implementation originated from
`https://github.com/rodrigosetti/corewar`. This example vendors files from DRQ,
not directly from the original repository.
