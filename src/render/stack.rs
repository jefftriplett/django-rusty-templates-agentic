//! Stops runaway template recursion before it overflows the native stack.
//!
//! Django renders nested templates with Python frames, so a template that
//! includes itself ends in `RecursionError`. Native rendering uses the
//! thread's stack directly, and overflowing it kills the process.

use pyo3::exceptions::PyRecursionError;
use pyo3::prelude::*;

/// Python frames Django uses for each nested template render, roughly:
/// `IncludeNode.render`, `Template.render`, `_render`, `NodeList.render` and
/// `render_annotated`. Dividing Python's recursion limit by this gives about
/// the depth at which Django fails.
const FRAMES_PER_RENDER: i32 = 5;

/// Stack kept free for the work inside the innermost template: its nested
/// tags, filters and the Python code they call.
const RESERVE: usize = 128 * 1024;

/// How deep a render is, kept on its `Context` rather than per thread so
/// greenlets that switch in the middle of a render don't share a count.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStack {
    depth: i32,
    /// Stack left when the outermost template started.
    top_remaining: usize,
    /// Stack left when the innermost template started.
    last_remaining: usize,
    /// The most stack one level of nesting has used so far.
    largest_level: usize,
}

impl RenderStack {
    /// Enters a nested template render. Returns the state to give back to
    /// `leave`, or `RecursionError` when the render is as deep as Django
    /// allows or another level might not fit in the stack.
    pub fn enter(&mut self) -> PyResult<Self> {
        let previous = *self;
        // SAFETY: `Py_GetRecursionLimit` only reads the interpreter's limit.
        // Rendering always holds a `Python` token, so the interpreter exists.
        let limit = unsafe { pyo3::ffi::Py_GetRecursionLimit() };
        if self.depth >= limit / FRAMES_PER_RENDER {
            return Err(Self::error());
        }
        self.depth += 1;
        // `None` on platforms `stacker` can't measure; the count still applies.
        if let Some(remaining) = stacker::remaining_stack() {
            if self.depth == 1 {
                self.top_remaining = remaining;
            } else {
                let level = self.last_remaining.saturating_sub(remaining);
                self.largest_level = self.largest_level.max(level);
                // A small thread stack keeps a quarter free rather than `RESERVE`.
                let reserve = RESERVE.min(self.top_remaining / 4);
                if remaining < reserve.saturating_add(self.largest_level.saturating_mul(2)) {
                    *self = previous;
                    return Err(Self::error());
                }
            }
            self.last_remaining = remaining;
        }
        Ok(previous)
    }

    /// Leaves the render `enter` started, keeping what it learned about how
    /// much stack a level uses.
    pub fn leave(&mut self, previous: Self) {
        let largest_level = self.largest_level.max(previous.largest_level);
        *self = Self {
            largest_level,
            ..previous
        };
    }

    fn error() -> PyErr {
        PyRecursionError::new_err("maximum recursion depth exceeded while rendering a template")
    }
}
