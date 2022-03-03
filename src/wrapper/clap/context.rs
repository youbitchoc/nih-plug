use atomic_refcell::AtomicRefMut;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::wrapper::{OutputParamChange, Task, Wrapper};
use crate::context::ProcessContext;
use crate::event_loop::EventLoop;
use crate::param::internals::ParamPtr;
use crate::plugin::{ClapPlugin, NoteEvent};
use crate::GuiContext;

/// A [`GuiContext`] implementation for the wrapper. This is passed to the plugin in
/// [`Editor::spawn()`][crate::Editor::spawn()] so it can interact with the rest of the plugin and
/// with the host for things like setting parameters.
pub(crate) struct WrapperGuiContext<P: ClapPlugin> {
    pub(super) wrapper: Arc<Wrapper<P>>,
}

/// A [`ProcessContext`] implementation for the wrapper. This is a separate object so it can hold on
/// to lock guards for event queues. Otherwise reading these events would require constant
/// unnecessary atomic operations to lock the uncontested RwLocks.
pub(crate) struct WrapperProcessContext<'a, P: ClapPlugin> {
    pub(super) wrapper: &'a Wrapper<P>,
    pub(super) input_events_guard: AtomicRefMut<'a, VecDeque<NoteEvent>>,
}

impl<P: ClapPlugin> GuiContext for WrapperGuiContext<P> {
    // All of these functions are supposed to be called from the main thread, so we'll put some
    // trust in the caller and assume that this is indeed the case
    unsafe fn raw_begin_set_parameter(&self, _param: ParamPtr) {
        // TODO: Parameter event gestures are a bit weird in CLAP right now because they're
        //       implemented as flags on events, and you don't know when a gesture ends before it
        //       has ended. Implement this once that's a bit clearer.
    }

    unsafe fn raw_set_parameter_normalized(&self, param: ParamPtr, normalized: f32) {
        match self.wrapper.param_ptr_to_hash.get(&param) {
            Some(hash) => {
                // We queue the parameter change event here, and it will be sent to the host either
                // at the end of the current processing cycle or after requesting an explicit flush
                // (when the plugin isn't processing audio). The parameter's actual value will only
                // be changed when the output event is written to prevent changing parameter values
                // in the middle of processing audio.
                let clap_plain_value = normalized as f64 * param.step_count().unwrap_or(1) as f64;
                let success = self.wrapper.queue_parameter_change(OutputParamChange {
                    param_hash: *hash,
                    clap_plain_value,
                });

                nih_debug_assert!(success, "Parameter output queue was full, parameter change will not be sent to the host");
            }
            None => nih_debug_assert_failure!("Unknown parameter: {:?}", param),
        }
    }

    unsafe fn raw_end_set_parameter(&self, _param: ParamPtr) {
        // TODO: Parameter event gestures are a bit weird in CLAP right now because they're
        //       implemented as flags on events, and you don't know when a gesture ends before it
        //       has ended. Implement this once that's a bit clearer.
    }

    unsafe fn raw_default_normalized_param_value(&self, param: ParamPtr) -> f32 {
        match self.wrapper.param_ptr_to_hash.get(&param) {
            Some(hash) => self.wrapper.param_defaults_normalized[hash],
            None => {
                nih_debug_assert_failure!("Unknown parameter: {:?}", param);
                0.5
            }
        }
    }
}

impl<P: ClapPlugin> ProcessContext for WrapperProcessContext<'_, P> {
    fn set_latency_samples(&self, samples: u32) {
        // Only make a callback if it's actually needed
        // XXX: For CLAP we could move this handling to the Plugin struct, but it may be worthwhile
        //      to keep doing it this way to stay consistent with VST3.
        let old_latency = self.wrapper.current_latency.swap(samples, Ordering::SeqCst);
        if old_latency != samples {
            let task_posted = self.wrapper.do_maybe_async(Task::LatencyChanged);
            nih_debug_assert!(task_posted, "The task queue is full, dropping task...");
        }
    }

    fn next_midi_event(&mut self) -> Option<NoteEvent> {
        self.input_events_guard.pop_front()
    }
}
