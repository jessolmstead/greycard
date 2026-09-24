# Tools for a pure-move split of the UI

Written for the `app.slint` split (notes §154), kept for the next move
of that kind.

- `inline.py`: puts every panel component instance in the branch's
  `app.slint` back into `App` and writes the result, so it can be
  diffed against master's `app.slint` with whitespace stripped. Also
  reports every instance binding that is not an identity forward.
- `gen_difftest.py`: generates `src/difftest.rs`, a differential fuzz
  of `App`'s tree on the testing backend. The same generated file goes
  into an export of master and of the branch; each drives the window
  with a seeded sequence of pointer, wheel and key events and writes
  every readable public property that changed and every callback that
  fired after each event. The two traces must be identical if the
  split is a pure move.

Both take their paths on the command line; read their docstrings.
