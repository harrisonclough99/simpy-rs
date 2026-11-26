use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Mutex;

use pyo3::exceptions::{PyRuntimeError, PyStopIteration, PyValueError};
use pyo3::prelude::*;

use crate::event::{AllOf, AnyOf, Condition, Event, InternalCallback, Timeout, NORMAL, URGENT};
use crate::exceptions::{EmptySchedule, StopSimulation};
use crate::process::Process;
use crate::resources::{
    Container, FilterStore, PreemptiveResource, PriorityResource, PriorityStore, Resource, Store,
};

struct HeapEntry {
    time: f64,
    priority: u8,
    eid: u64,
    event: Py<Event>,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.time.to_bits() == other.time.to_bits()
            && self.priority == other.priority
            && self.eid == other.eid
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering so BinaryHeap behaves as a min-heap on (time, priority, eid).
        other
            .time
            .total_cmp(&self.time)
            .then_with(|| other.priority.cmp(&self.priority))
            .then_with(|| other.eid.cmp(&self.eid))
    }
}

struct EnvInner {
    now: f64,
    eid: u64,
    queue: BinaryHeap<HeapEntry>,
    active_process: Option<Py<Process>>,
}

enum StepResult {
    Continue,
    Empty,
    Stop(Py<PyAny>),
}

/// Execution environment for a discrete-event simulation.
#[pyclass(subclass, module = "simpy_rs._simpy_rs")]
pub struct Environment {
    inner: Mutex<EnvInner>,
}

impl Environment {
    pub fn now_rs(&self) -> f64 {
        self.inner.lock().unwrap().now
    }

    pub fn next_eid(&self) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        let id = inner.eid;
        inner.eid += 1;
        id
    }

    pub fn set_active_process(&self, proc_opt: Option<Py<Process>>) {
        self.inner.lock().unwrap().active_process = proc_opt;
    }

    pub fn active_process_rs(&self, py: Python<'_>) -> Option<Py<Process>> {
        self.inner
            .lock()
            .unwrap()
            .active_process
            .as_ref()
            .map(|p| p.clone_ref(py))
    }

    pub fn is_active_process(&self, proc_py: &Py<Process>) -> bool {
        let inner = self.inner.lock().unwrap();
        match &inner.active_process {
            Some(active) => active.is(proc_py),
            None => false,
        }
    }

    pub fn schedule_rs(&self, event: Py<Event>, priority: u8, delay: f64) -> PyResult<()> {
        if delay.is_nan() || delay < 0.0 {
            return Err(PyValueError::new_err(format!("Negative delay {delay}")));
        }
        let mut inner = self.inner.lock().unwrap();
        let time = inner.now + delay;
        let eid = inner.eid;
        inner.eid += 1;
        inner.queue.push(HeapEntry {
            time,
            priority,
            eid,
            event,
        });
        Ok(())
    }

    pub fn peek_rs(&self) -> f64 {
        let inner = self.inner.lock().unwrap();
        inner.queue.peek().map(|e| e.time).unwrap_or(f64::INFINITY)
    }

    #[inline]
    fn dispatch_internal_cb(
        py: Python<'_>,
        event_py: &Py<Event>,
        cb: InternalCallback,
    ) -> PyResult<Option<Py<PyAny>>> {
        match cb {
            InternalCallback::Process(proc_py) => {
                Process::resume_rs(&proc_py, py, event_py)?;
            }
            InternalCallback::StopSimulation => {
                let (ok, val) = {
                    let ev = event_py.borrow(py);
                    let inner = ev.inner.lock().unwrap();
                    (
                        inner.ok,
                        inner
                            .value
                            .as_ref()
                            .map(|v| v.clone_ref(py))
                            .unwrap_or_else(|| py.None()),
                    )
                };
                if ok {
                    return Ok(Some(val));
                } else {
                    return Err(PyErr::from_value(val.into_bound(py)));
                }
            }
            InternalCallback::Condition(cond_py) => {
                Condition::check_rs(&cond_py, py, event_py)?;
            }
            InternalCallback::ResourcePut(res) => {
                Resource::trigger_put_rs(&res, py)?;
            }
            InternalCallback::ResourceGet(res) => {
                Resource::trigger_get_rs(&res, py)?;
            }
            InternalCallback::PriorityResourcePut(res) => {
                PriorityResource::trigger_put_rs(&res, py)?;
            }
            InternalCallback::PriorityResourceGet(res) => {
                PriorityResource::trigger_get_rs(&res, py)?;
            }
            InternalCallback::PreemptiveResourcePut(res) => {
                PreemptiveResource::trigger_put_rs(&res, py)?;
            }
            InternalCallback::PreemptiveResourceGet(res) => {
                PreemptiveResource::trigger_get_rs(&res, py)?;
            }
            InternalCallback::ContainerPut(cont) => {
                Container::trigger_put_rs(&cont, py)?;
            }
            InternalCallback::ContainerGet(cont) => {
                Container::trigger_get_rs(&cont, py)?;
            }
            InternalCallback::StorePut(store) => {
                Store::trigger_put_rs(&store, py)?;
            }
            InternalCallback::StoreGet(store) => {
                Store::trigger_get_rs(&store, py)?;
            }
            InternalCallback::PriorityStorePut(store) => {
                PriorityStore::trigger_put_rs(&store, py)?;
            }
            InternalCallback::PriorityStoreGet(store) => {
                PriorityStore::trigger_get_rs(&store, py)?;
            }
            InternalCallback::FilterStorePut(store) => {
                FilterStore::trigger_put_rs(&store, py)?;
            }
            InternalCallback::FilterStoreGet(store) => {
                FilterStore::trigger_get_rs(&store, py)?;
            }
        }
        Ok(None)
    }

    fn step_internal(&self, py: Python<'_>) -> PyResult<StepResult> {
        let event_py = {
            let mut inner = self.inner.lock().unwrap();
            match inner.queue.pop() {
                Some(entry) => {
                    inner.now = entry.time;
                    entry.event
                }
                None => return Ok(StepResult::Empty),
            }
        };

        let (first_cb, extra_cbs, py_cbs) =
            event_py.borrow(py).take_callbacks_and_mark_processed(py);

        if let Some(cb) = first_cb {
            if let Some(stop_val) = Self::dispatch_internal_cb(py, &event_py, cb)? {
                return Ok(StepResult::Stop(stop_val));
            }
        }

        for cb in extra_cbs {
            if let Some(stop_val) = Self::dispatch_internal_cb(py, &event_py, cb)? {
                return Ok(StepResult::Stop(stop_val));
            }
        }

        for py_cb in py_cbs {
            if let Err(err) = py_cb.bind(py).call1((event_py.bind(py),)) {
                if err.is_instance_of::<StopSimulation>(py) {
                    let val = err
                        .value(py)
                        .getattr("value")
                        .map(|v| v.unbind())
                        .unwrap_or_else(|_| py.None());
                    return Ok(StepResult::Stop(val));
                }
                return Err(err);
            }
        }

        // If the event failed and was not defused by any callback, re-raise its exception.
        let un_defused_err = {
            let ev = event_py.borrow(py);
            let inner = ev.inner.lock().unwrap();
            if !inner.ok && !inner.defused {
                inner.value.as_ref().map(|v| v.clone_ref(py))
            } else {
                None
            }
        };
        if let Some(exc) = un_defused_err {
            return Err(PyErr::from_value(exc.into_bound(py)));
        }

        Ok(StepResult::Continue)
    }
}

#[pymethods]
impl Environment {
    #[new]
    #[pyo3(signature = (initial_time = 0.0))]
    fn py_new(initial_time: f64) -> PyResult<Self> {
        if initial_time.is_nan() {
            return Err(PyValueError::new_err("initial_time must not be NaN"));
        }
        Ok(Self {
            inner: Mutex::new(EnvInner {
                now: initial_time,
                eid: 0,
                queue: BinaryHeap::new(),
                active_process: None,
            }),
        })
    }

    #[getter]
    fn now(&self) -> f64 {
        self.now_rs()
    }

    #[getter]
    fn active_process(&self, py: Python<'_>) -> Option<Py<Process>> {
        self.active_process_rs(py)
    }

    fn event(slf: Bound<'_, Self>, py: Python<'_>) -> PyResult<Py<Event>> {
        Py::new(py, Event::new_pending(slf.unbind()))
    }

    #[pyo3(signature = (delay = 0.0, value = None))]
    fn timeout(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        delay: f64,
        value: Option<Py<PyAny>>,
    ) -> PyResult<Py<Timeout>> {
        Timeout::create_rs(py, slf.unbind(), delay, value)
    }

    fn process(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        generator: Bound<'_, PyAny>,
    ) -> PyResult<Py<Process>> {
        Process::create_rs(py, slf.unbind(), generator)
    }

    fn all_of(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        events: Bound<'_, PyAny>,
    ) -> PyResult<Py<AllOf>> {
        let mut evs = Vec::new();
        for item in events.try_iter()? {
            let obj = item?;
            let ev = obj.cast::<Event>().map_err(|_| {
                PyValueError::new_err(format!("Expected Event instance, got {obj:?}"))
            })?;
            evs.push(ev.clone().unbind());
        }
        AllOf::create_rs(py, slf.unbind(), evs)
    }

    fn any_of(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        events: Bound<'_, PyAny>,
    ) -> PyResult<Py<AnyOf>> {
        let mut evs = Vec::new();
        for item in events.try_iter()? {
            let obj = item?;
            let ev = obj.cast::<Event>().map_err(|_| {
                PyValueError::new_err(format!("Expected Event instance, got {obj:?}"))
            })?;
            evs.push(ev.clone().unbind());
        }
        AnyOf::create_rs(py, slf.unbind(), evs)
    }

    #[pyo3(signature = (event, priority = NORMAL, delay = 0.0))]
    fn schedule(&self, event: Py<Event>, priority: u8, delay: f64) -> PyResult<()> {
        self.schedule_rs(event, priority, delay)
    }

    fn peek(&self) -> f64 {
        self.peek_rs()
    }

    fn step(&self, py: Python<'_>) -> PyResult<()> {
        match self.step_internal(py)? {
            StepResult::Continue => Ok(()),
            StepResult::Empty => Err(EmptySchedule::new_err(())),
            StepResult::Stop(val) => Err(StopSimulation::new_err((val,))),
        }
    }

    #[pyo3(signature = (until = None))]
    fn run(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        until: Option<Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let (until_event, is_numeric_until): (Option<Py<Event>>, bool) = match until {
            None => (None, false),
            Some(ref u) => {
                if let Ok(ev_bound) = u.cast::<Event>() {
                    let ev_py: Py<Event> = ev_bound.clone().unbind();
                    if ev_py.borrow(py).is_processed() {
                        let (ok, val) = {
                            let ev = ev_py.borrow(py);
                            let inner = ev.inner.lock().unwrap();
                            (
                                inner.ok,
                                inner
                                    .value
                                    .as_ref()
                                    .map(|v| v.clone_ref(py))
                                    .unwrap_or_else(|| py.None()),
                            )
                        };
                        return if ok {
                            Ok(val)
                        } else {
                            Err(PyErr::from_value(val.into_bound(py)))
                        };
                    }
                    ev_py
                        .borrow(py)
                        .add_internal_callback(InternalCallback::StopSimulation);
                    (Some(ev_py), false)
                } else {
                    let at: f64 = u.extract()?;
                    let now = slf.borrow().now_rs();
                    if at.is_nan() || at <= now {
                        return Err(PyValueError::new_err(format!(
                            "until(={at}) must be > the current simulation time."
                        )));
                    }
                    let stop_ev = Py::new(
                        py,
                        Event::new_triggered(slf.clone().unbind(), true, py.None()),
                    )?;
                    stop_ev
                        .borrow(py)
                        .add_internal_callback(InternalCallback::StopSimulation);
                    slf.borrow()
                        .schedule_rs(stop_ev.clone_ref(py), URGENT, at - now)?;
                    (Some(stop_ev), true)
                }
            }
        };

        let env_ref = slf.borrow();
        loop {
            match env_ref.step_internal(py)? {
                StepResult::Continue => continue,
                StepResult::Stop(val) => {
                    return Ok(if is_numeric_until { py.None() } else { val });
                }
                StepResult::Empty => {
                    if let Some(ref u_ev) = until_event {
                        let (triggered, ok, val) = {
                            let ev = u_ev.borrow(py);
                            let inner = ev.inner.lock().unwrap();
                            (
                                inner.triggered,
                                inner.ok,
                                inner
                                    .value
                                    .as_ref()
                                    .map(|v| v.clone_ref(py))
                                    .unwrap_or_else(|| py.None()),
                            )
                        };
                        if !triggered {
                            let repr = u_ev.bind(py).repr()?;
                            return Err(PyRuntimeError::new_err(format!(
                                "No scheduled events left but \"until\" event was not triggered: {repr}"
                            )));
                        }
                        return if ok {
                            Ok(if is_numeric_until { py.None() } else { val })
                        } else {
                            Err(PyErr::from_value(val.into_bound(py)))
                        };
                    }
                    return Ok(py.None());
                }
            }
        }
    }

    #[pyo3(signature = (value = None))]
    fn exit(&self, value: Option<Py<PyAny>>) -> PyResult<()> {
        match value {
            Some(v) => Err(PyStopIteration::new_err((v,))),
            None => Err(PyStopIteration::new_err(())),
        }
    }
}
