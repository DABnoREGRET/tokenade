//! The SHAPES a value takes in a tool's output.
//!
//! A compactor that groups two texts that are "basically identical" must
//! first decide what, in a line, is a value and what is the
//! sentence. Two places decide it: [`crate::template_fold`], which
//! CAPTURES the values to list them again under the template, and
//! [`crate::stacktrace`], which REPLACES them with a token to give a key
//! to two structurally identical traces.
//!
//! The two mechanisms differ and must differ. **The spelling of the
//! shape, however, must not**: it did, and the two did not say the
//! same thing — `stacktrace` wrote `0x[0-9a-fA-F]+`, which misses
//! `0XDEADBEEF` (the capital is legal in Java as in C) and catches the
//! end of an identifier like `foo0xff`.
