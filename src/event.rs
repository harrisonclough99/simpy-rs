use std::sync::Mutex;

use pyo3::exceptions::{PyAttributeError, PyBaseException, PyKeyError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};

use crate::env::Environment;
use crate::process::Process;
use crate::resources::{
    Container, FilterStore, PreemptiveResource, PriorityResource, PriorityStore, Resource, Store,
};

pub const URGENT: u8 = 0;
pub const NORMAL: u8 = 1;

/// Sentinel object matching `simpy.events.PENDING`.
#[pyclass(module = "simpy_rs._simpy_rs")]
pub struct Pending;

#[pymethods]
impl Pending {
    fn __repr__(&self) -> &'static str {
        "<PENDING>"
    }
}

/// Fast Rust-internal callback variants that avoid Python bound-method allocations.
pub enum InternalCallback {
    Process(Py<Process>),
    StopSimulation,
    Condition(Py<Condition>),
    ResourcePut(Py<Resource>),
    ResourceGet(Py<Resource>),
    PriorityResourcePut(Py<PriorityResource>),
    PriorityResourceGet(Py<PriorityResource>),
    PreemptiveResourcePut(Py<PreemptiveResource>),
    PreemptiveResourceGet(Py<PreemptiveResource>),
    ContainerPut(Py<Container>),
    ContainerGet(Py<Container>),
    StorePut(Py<Store>),
    StoreGet(Py<Store>),
    PriorityStorePut(Py<PriorityStore>),
    PriorityStoreGet(Py<PriorityStore>),
    FilterStorePut(Py<FilterStore>),
    FilterStoreGet(Py<FilterStore>),
}

pub struct EventInner {
    pub ok: bool,
    pub triggered: bool,
    pub processed: bool,
    pub defused: bool,
    pub is_interruption: bool,
    pub value: Option<Py<PyAny>>,
    /// Inline slot for the common single-callback case (avoids Vec heap allocation).
    pub first_cb: Option<InternalCallback>,
    pub extra_cbs: Vec<InternalCallback>,
    pub py_callbacks: Option<Py<PyList>>,
}

impl EventInner {
    pub fn new_pending() -> Self {
        Self {
            ok: false,
            triggered: false,
            processed: false,
            defused: false,
            is_interruption: false,
            value: None,
            first_cb: None,
            extra_cbs: Vec::new(),
            py_callbacks: None,
        }
    }

    pub fn new_triggered(ok: bool, value: Py<PyAny>) -> Self {
        Self {
            ok,
            triggered: true,
            processed: false,
            defused: false,
            is_interruption: false,
            value: Some(value),
            first_cb: None,
            extra_cbs: Vec::new(),
            py_callbacks: None,
        }
    }
}

/// Base class for all events in `simpy-rs`.
#[pyclass(subclass, module = "simpy_rs._simpy_rs")]
pub struct Event {
    pub env: Py<Environment>,
    pub inner: Mutex<EventInner>,
}

impl Event {
    pub fn new_pending(env: Py<Environment>) -> Self {
        Self {
            env,
            inner: Mutex::new(EventInner::new_pending()),
        }
    }

    pub fn new_triggered(env: Py<Environment>, ok: bool, value: Py<PyAny>) -> Self {
        Self {
            env,
            inner: Mutex::new(EventInner::new_triggered(ok, value)),
        }
    }

    pub fn new_interruption(env: Py<Environment>, exc: Py<PyAny>) -> Self {
        let mut inner = EventInner::new_triggered(false, exc);
        inner.defused = true;
        inner.is_interruption = true;
        Self {
            env,
            inner: Mutex::new(inner),
        }
    }

    #[inline]
    pub fn add_internal_callback(&self, cb: InternalCallback) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.processed {
            return false;
        }
        if inner.first_cb.is_none() {
            inner.first_cb = Some(cb);
        } else {
            inner.extra_cbs.push(cb);
        }
        true
    }

    pub fn remove_process_callback(&self, proc_obj: &Py<Process>) {
        let mut inner = self.inner.lock().unwrap();
        if !inner.processed {
            if let Some(InternalCallback::Process(ref p)) = inner.first_cb {
                if p.is(proc_obj) {
                    inner.first_cb = None;
                }
            }
            if !inner.extra_cbs.is_empty() {
                inner.extra_cbs.retain(|cb| match cb {
                    InternalCallback::Process(p) => !p.is(proc_obj),
                    _ => true,
                });
            }
        }
    }

    pub fn is_processed(&self) -> bool {
        self.inner.lock().unwrap().processed
    }

    pub fn is_triggered(&self) -> bool {
        self.inner.lock().unwrap().triggered
    }

    pub fn is_ok(&self) -> bool {
        self.inner.lock().unwrap().ok
    }

    pub fn set_defused_rs(&self, val: bool) {
        self.inner.lock().unwrap().defused = val;
    }

    pub fn clone_value(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.inner
            .lock()
            .unwrap()
            .value
            .as_ref()
            .map(|v| v.clone_ref(py))
    }

    /// Take all callbacks and mark this event as processed.
    #[inline]
    pub fn take_callbacks_and_mark_processed(
        &self,
        py: Python<'_>,
    ) -> (Option<InternalCallback>, Vec<InternalCallback>, Vec<Py<PyAny>>) {
        let mut inner = self.inner.lock().unwrap();
        inner.processed = true;
        let first = inner.first_cb.take();
        let extra = if inner.extra_cbs.is_empty() {
            Vec::new()
        } else {
            std::mem::take(&mut inner.extra_cbs)
        };
        let py_cbs = if let Some(list) = inner.py_callbacks.take() {
            let bound = list.bind(py);
            let mut out = Vec::with_capacity(bound.len());
            for item in bound.iter() {
                out.push(item.unbind());
            }
            out
        } else {
            Vec::new()
        };
        (first, extra, py_cbs)
    }

    /// Succeed this event from Rust and schedule it on its environment.
    pub fn succeed_rs(event_py: &Py<Event>, py: Python<'_>, value: Py<PyAny>) -> PyResult<()> {
        let env = {
            let ev = event_py.borrow(py);
            let mut inner = ev.inner.lock().unwrap();
            if inner.triggered {
                return Err(PyRuntimeError::new_err("Event has already been triggered"));
            }
            inner.ok = true;
            inner.triggered = true;
            inner.value = Some(value);
            ev.env.clone_ref(py)
        };
        env.borrow(py)
            .schedule_rs(event_py.clone_ref(py), NORMAL, 0.0)?;
        Ok(())
    }

    /// Fail this event from Rust and schedule it on its environment.
    pub fn fail_rs(event_py: &Py<Event>, py: Python<'_>, exception: Py<PyAny>) -> PyResult<()> {
        if !exception.bind(py).is_instance_of::<PyBaseException>() {
            return Err(PyValueError::new_err(format!(
                "{:?} is not an exception.",
                exception.bind(py)
            )));
        }
        let env = {
            let ev = event_py.borrow(py);
            let mut inner = ev.inner.lock().unwrap();
            if inner.triggered {
                return Err(PyRuntimeError::new_err("Event has already been triggered"));
            }
            inner.ok = false;
            inner.triggered = true;
            inner.value = Some(exception);
            ev.env.clone_ref(py)
        };
        env.borrow(py)
            .schedule_rs(event_py.clone_ref(py), NORMAL, 0.0)?;
        Ok(())
    }
}

#[pymethods]
impl Event {
    #[new]
    fn py_new(env: Py<Environment>) -> Self {
        Self::new_pending(env)
    }

    #[getter]
    fn env(&self, py: Python<'_>) -> Py<Environment> {
        self.env.clone_ref(py)
    }

    #[getter]
    fn triggered(&self) -> bool {
        self.inner.lock().unwrap().triggered
    }

    #[getter]
    fn processed(&self) -> bool {
        self.inner.lock().unwrap().processed
    }

    #[getter]
    fn ok(&self) -> PyResult<bool> {
        let inner = self.inner.lock().unwrap();
        if !inner.triggered {
            return Err(PyAttributeError::new_err("Event is not yet triggered"));
        }
        Ok(inner.ok)
    }

    #[getter]
    fn value(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let inner = self.inner.lock().unwrap();
        match &inner.value {
            Some(v) => Ok(v.clone_ref(py)),
            None => Err(PyAttributeError::new_err(
                "Value of event is not yet available",
            )),
        }
    }

    #[getter]
    fn defused(&self) -> bool {
        self.inner.lock().unwrap().defused
    }

    #[setter]
    fn set_defused(&self, val: bool) {
        self.inner.lock().unwrap().defused = val;
    }

    #[getter]
    fn callbacks(&self, py: Python<'_>) -> PyResult<Option<Py<PyList>>> {
        let mut inner = self.inner.lock().unwrap();
        if inner.processed {
            return Ok(None);
        }
        if inner.py_callbacks.is_none() {
            inner.py_callbacks = Some(PyList::empty(py).unbind());
        }
        Ok(inner.py_callbacks.as_ref().map(|l| l.clone_ref(py)))
    }

    #[setter]
    fn set_callbacks(&self, val: Option<Py<PyList>>) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(list) = val {
            inner.processed = false;
            inner.py_callbacks = Some(list);
        } else {
            inner.processed = true;
            inner.py_callbacks = None;
            inner.first_cb = None;
            inner.extra_cbs.clear();
        }
    }

    #[pyo3(signature = (value = None))]
    fn succeed(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        value: Option<Py<PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let val = value.unwrap_or_else(|| py.None());
        let ev_py: Py<Event> = slf.clone().unbind();
        Event::succeed_rs(&ev_py, py, val)?;
        Ok(slf.into_any().unbind())
    }

    fn fail(slf: Bound<'_, Self>, py: Python<'_>, exception: Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let ev_py: Py<Event> = slf.clone().unbind();
        Event::fail_rs(&ev_py, py, exception.unbind())?;
        Ok(slf.into_any().unbind())
    }

    fn trigger(slf: Bound<'_, Self>, py: Python<'_>, event: Bound<'_, Event>) -> PyResult<()> {
        let (ok, val) = {
            let other = event.borrow();
            let inner = other.inner.lock().unwrap();
            if !inner.triggered {
                return Err(PyAttributeError::new_err(
                    "Source event is not yet triggered",
                ));
            }
            (
                inner.ok,
                inner
                    .value
                    .as_ref()
                    .map(|v| v.clone_ref(py))
                    .unwrap_or_else(|| py.None()),
            )
        };
        let env = {
            let me = slf.borrow();
            let mut inner = me.inner.lock().unwrap();
            if inner.triggered {
                return Err(PyRuntimeError::new_err("Event has already been triggered"));
            }
            inner.ok = ok;
            inner.triggered = true;
            inner.value = Some(val);
            me.env.clone_ref(py)
        };
        env.borrow(py)
            .schedule_rs(slf.unbind(), NORMAL, 0.0)?;
        Ok(())
    }

    fn __and__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        other: Bound<'_, Event>,
    ) -> PyResult<Py<AllOf>> {
        let env = slf.borrow().env.clone_ref(py);
        let events = vec![slf.unbind(), other.unbind()];
        AllOf::create_rs(py, env, events)
    }

    fn __or__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        other: Bound<'_, Event>,
    ) -> PyResult<Py<AnyOf>> {
        let env = slf.borrow().env.clone_ref(py);
        let events = vec![slf.unbind(), other.unbind()];
        AnyOf::create_rs(py, env, events)
    }

    fn __repr__(slf: Bound<'_, Self>) -> PyResult<String> {
        let cls = slf.get_type().name()?;
        let ptr = slf.as_ptr() as usize;
        Ok(format!("<{cls}() object at {ptr:#x}>"))
    }
}

/// Event that is triggered after a given `delay` has passed.
#[pyclass(extends = Event, subclass, module = "simpy_rs._simpy_rs")]
pub struct Timeout {
    pub delay: f64,
}

impl Timeout {
    pub fn create_rs(
        py: Python<'_>,
        env: Py<Environment>,
        delay: f64,
        value: Option<Py<PyAny>>,
    ) -> PyResult<Py<Timeout>> {
        if delay.is_nan() || delay < 0.0 {
            return Err(PyValueError::new_err(format!("Negative delay {delay}")));
        }
        let val = value.unwrap_or_else(|| py.None());
        let env_ref = env.clone_ref(py);
        let timeout = Py::new(
            py,
            PyClassInitializer::from(Event::new_triggered(env, true, val))
                .add_subclass(Timeout { delay }),
        )?;
        let ev_py: Py<Event> = timeout.bind(py).as_super().clone().unbind();
        env_ref.borrow(py).schedule_rs(ev_py, NORMAL, delay)?;
        Ok(timeout)
    }
}

#[pymethods]
impl Timeout {
    #[new]
    #[pyo3(signature = (env, delay, value = None))]
    fn py_new(
        py: Python<'_>,
        env: Py<Environment>,
        delay: f64,
        value: Option<Py<PyAny>>,
    ) -> PyResult<PyClassInitializer<Self>> {
        if delay.is_nan() || delay < 0.0 {
            return Err(PyValueError::new_err(format!("Negative delay {delay}")));
        }
        let val = value.unwrap_or_else(|| py.None());
        Ok(
            PyClassInitializer::from(Event::new_triggered(env, true, val))
                .add_subclass(Timeout { delay }),
        )
    }

    #[pyo3(signature = (*_args, **_kwargs))]
    fn __init__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        _args: Bound<'_, PyTuple>,
        _kwargs: Option<Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        let delay = slf.borrow().delay;
        let ev = slf.as_super();
        let env = ev.borrow().env.clone_ref(py);
        env.borrow(py)
            .schedule_rs(ev.clone().unbind(), NORMAL, delay)?;
        Ok(())
    }

    fn __repr__(slf: Bound<'_, Self>) -> PyResult<String> {
        let delay = slf.borrow().delay;
        let ptr = slf.as_ptr() as usize;
        Ok(format!("<Timeout({delay}) object at {ptr:#x}>"))
    }
}

/// Internal event used to initialize a `Process`.
#[pyclass(extends = Event, module = "simpy_rs._simpy_rs")]
pub struct Initialize;

impl Initialize {
    pub fn create_and_schedule(
        py: Python<'_>,
        env: Py<Environment>,
        proc_py: Py<Process>,
    ) -> PyResult<Py<Initialize>> {
        let env_ref = env.clone_ref(py);
        let init_ev = Py::new(
            py,
            PyClassInitializer::from(Event::new_triggered(env, true, py.None()))
                .add_subclass(Initialize),
        )?;
        {
            let ev_ref = init_ev.bind(py).as_super();
            ev_ref
                .borrow()
                .add_internal_callback(InternalCallback::Process(proc_py));
        }
        let ev_py: Py<Event> = init_ev.bind(py).as_super().clone().unbind();
        env_ref.borrow(py).schedule_rs(ev_py, URGENT, 0.0)?;
        Ok(init_ev)
    }
}

/// Internal event used to interrupt a `Process`.
#[pyclass(extends = Event, module = "simpy_rs._simpy_rs")]
pub struct Interruption;

impl Interruption {
    pub fn create_and_schedule(
        py: Python<'_>,
        env: Py<Environment>,
        proc_py: Py<Process>,
        cause: Py<PyAny>,
    ) -> PyResult<Py<Interruption>> {
        let interrupt_exc = py
            .get_type::<crate::exceptions::Interrupt>()
            .call1((cause,))?
            .unbind();
        let env_ref = env.clone_ref(py);
        let inter = Py::new(
            py,
            PyClassInitializer::from(Event::new_interruption(env, interrupt_exc))
                .add_subclass(Interruption),
        )?;
        {
            let ev_ref = inter.bind(py).as_super();
            ev_ref
                .borrow()
                .add_internal_callback(InternalCallback::Process(proc_py));
        }
        let ev_py: Py<Event> = inter.bind(py).as_super().clone().unbind();
        env_ref.borrow(py).schedule_rs(ev_py, URGENT, 0.0)?;
        Ok(inter)
    }
}

/// Result value of a `Condition` event (`AllOf` / `AnyOf`), behaving like a dict of `{event: value}`.
#[pyclass(module = "simpy_rs._simpy_rs")]
pub struct ConditionValue {
    pub events: Vec<Py<Event>>,
}

impl ConditionValue {
    pub fn new(events: Vec<Py<Event>>) -> Self {
        Self { events }
    }

    pub fn to_py_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        for ev in &self.events {
            let val = ev
                .borrow(py)
                .clone_value(py)
                .unwrap_or_else(|| py.None());
            dict.set_item(ev.bind(py), val)?;
        }
        Ok(dict)
    }
}

#[pymethods]
impl ConditionValue {
    #[getter]
    fn events(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let items: Vec<&Bound<'_, Event>> = self.events.iter().map(|e| e.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    fn __getitem__(&self, py: Python<'_>, key: Bound<'_, Event>) -> PyResult<Py<PyAny>> {
        for ev in &self.events {
            if ev.bind(py).is(&key) {
                return Ok(ev
                    .borrow(py)
                    .clone_value(py)
                    .unwrap_or_else(|| py.None()));
            }
        }
        Err(PyKeyError::new_err(key.unbind()))
    }

    fn __contains__(&self, py: Python<'_>, key: Bound<'_, PyAny>) -> bool {
        self.events.iter().any(|ev| ev.bind(py).is(&key))
    }

    fn __len__(&self) -> usize {
        self.events.len()
    }

    fn __iter__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let list = self.events(py)?;
        Ok(list.bind(py).call_method0("__iter__")?.unbind())
    }

    fn keys(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.__iter__(py)
    }

    fn values(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let vals: Vec<Py<PyAny>> = self
            .events
            .iter()
            .map(|ev| ev.borrow(py).clone_value(py).unwrap_or_else(|| py.None()))
            .collect();
        let list = PyList::new(py, vals)?;
        Ok(list.call_method0("__iter__")?.unbind())
    }

    fn items(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let mut pairs = Vec::with_capacity(self.events.len());
        for ev in &self.events {
            let val = ev.borrow(py).clone_value(py).unwrap_or_else(|| py.None());
            let tup = PyTuple::new(py, &[ev.clone_ref(py).into_any(), val])?;
            pairs.push(tup);
        }
        let list = PyList::new(py, pairs)?;
        Ok(list.call_method0("__iter__")?.unbind())
    }

    fn todict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.to_py_dict(py)
    }

    fn __eq__(&self, py: Python<'_>, other: Bound<'_, PyAny>) -> PyResult<bool> {
        let self_dict = self.to_py_dict(py)?;
        if let Ok(other_cv) = other.cast::<ConditionValue>() {
            let other_dict = other_cv.borrow().to_py_dict(py)?;
            return self_dict.eq(other_dict);
        }
        self_dict.eq(other)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let dict = self.to_py_dict(py)?;
        let dict_repr = dict.repr()?;
        Ok(format!("<ConditionValue {dict_repr}>"))
    }
}

pub enum ConditionEval {
    All,
    Any,
    Custom(Py<PyAny>),
}

struct ConditionInner {
    count: usize,
}

/// Composite event that triggers when a condition over child events is satisfied.
#[pyclass(extends = Event, subclass, module = "simpy_rs._simpy_rs")]
pub struct Condition {
    pub eval: ConditionEval,
    pub events: Vec<Py<Event>>,
    inner: Mutex<ConditionInner>,
}

impl Condition {
    fn validate_events(py: Python<'_>, env: &Py<Environment>, events: &[Py<Event>]) -> PyResult<()> {
        for ev in events {
            let ev_env = &ev.borrow(py).env;
            if !ev_env.is(env) {
                return Err(PyValueError::new_err(format!(
                    "It is not allowed to mix events from different environments: {:?}",
                    ev.bind(py)
                )));
            }
        }
        Ok(())
    }

    fn collect_triggered_events(py: Python<'_>, events: &[Py<Event>], out: &mut Vec<Py<Event>>) {
        for ev in events {
            if ev.borrow(py).is_processed() {
                if let Ok(cond) = ev.bind(py).cast::<Condition>() {
                    let sub_events: Vec<Py<Event>> = cond
                        .borrow()
                        .events
                        .iter()
                        .map(|e| e.clone_ref(py))
                        .collect();
                    Self::collect_triggered_events(py, &sub_events, out);
                } else {
                    out.push(ev.clone_ref(py));
                }
            }
        }
    }

    fn eval_predicate(&self, py: Python<'_>, count: usize) -> PyResult<bool> {
        match &self.eval {
            ConditionEval::All => Ok(self.events.len() == count),
            ConditionEval::Any => Ok(count > 0 || self.events.is_empty()),
            ConditionEval::Custom(func) => {
                let ev_list: Vec<&Bound<'_, Event>> =
                    self.events.iter().map(|e| e.bind(py)).collect();
                let py_tup = PyTuple::new(py, ev_list)?;
                func.bind(py).call1((py_tup, count))?.is_truthy()
            }
        }
    }

    pub fn setup_condition(cond_py: &Py<Condition>, py: Python<'_>) -> PyResult<()> {
        let (events, env) = {
            let c = cond_py.borrow(py);
            let evs: Vec<Py<Event>> = c.events.iter().map(|e| e.clone_ref(py)).collect();
            let env = cond_py.bind(py).as_super().borrow().env.clone_ref(py);
            (evs, env)
        };

        Self::validate_events(py, &env, &events)?;

        if events.is_empty() {
            let ev_py: Py<Event> = cond_py.bind(py).as_super().clone().unbind();
            let cv = Py::new(py, ConditionValue::new(Vec::new()))?.into_any();
            Event::succeed_rs(&ev_py, py, cv)?;
            return Ok(());
        }

        for ev in &events {
            let processed = ev.borrow(py).is_processed();
            if processed {
                Self::check_rs(cond_py, py, ev)?;
                if cond_py.bind(py).as_super().borrow().is_triggered() {
                    break;
                }
            } else {
                ev.borrow(py)
                    .add_internal_callback(InternalCallback::Condition(cond_py.clone_ref(py)));
            }
        }
        Ok(())
    }

    pub fn check_rs(cond_py: &Py<Condition>, py: Python<'_>, child: &Py<Event>) -> PyResult<()> {
        let super_ev = cond_py.bind(py).as_super();
        if super_ev.borrow().is_triggered() {
            return Ok(());
        }

        let (child_ok, child_val) = {
            let ch = child.borrow(py);
            let inner = ch.inner.lock().unwrap();
            (
                inner.ok,
                inner
                    .value
                    .as_ref()
                    .map(|v| v.clone_ref(py))
                    .unwrap_or_else(|| py.None()),
            )
        };

        if !child_ok {
            child.borrow(py).set_defused_rs(true);
            let ev_py: Py<Event> = super_ev.clone().unbind();
            Event::fail_rs(&ev_py, py, child_val)?;
            return Ok(());
        }

        let new_count = {
            let c = cond_py.borrow(py);
            let mut inner = c.inner.lock().unwrap();
            inner.count += 1;
            inner.count
        };

        let satisfied = cond_py.borrow(py).eval_predicate(py, new_count)?;
        if satisfied {
            let events_snapshot: Vec<Py<Event>> = cond_py
                .borrow(py)
                .events
                .iter()
                .map(|e| e.clone_ref(py))
                .collect();
            let mut triggered_list = Vec::new();
            Self::collect_triggered_events(py, &events_snapshot, &mut triggered_list);
            let cv = Py::new(py, ConditionValue::new(triggered_list))?.into_any();
            let ev_py: Py<Event> = super_ev.clone().unbind();
            Event::succeed_rs(&ev_py, py, cv)?;
        }
        Ok(())
    }
}

fn extract_event_vec(py: Python<'_>, events: Bound<'_, PyAny>) -> PyResult<Vec<Py<Event>>> {
    let mut out = Vec::new();
    for item in events.try_iter()? {
        let obj = item?;
        let ev = obj.cast::<Event>().map_err(|_| {
            PyValueError::new_err(format!("Expected Event instance, got {obj:?}"))
        })?;
        out.push(ev.clone().unbind());
    }
    let _ = py;
    Ok(out)
}

#[pymethods]
impl Condition {
    #[new]
    fn py_new(
        py: Python<'_>,
        env: Py<Environment>,
        evaluate: Py<PyAny>,
        events: Bound<'_, PyAny>,
    ) -> PyResult<PyClassInitializer<Self>> {
        let evs = extract_event_vec(py, events)?;
        Ok(PyClassInitializer::from(Event::new_pending(env)).add_subclass(Condition {
            eval: ConditionEval::Custom(evaluate),
            events: evs,
            inner: Mutex::new(ConditionInner { count: 0 }),
        }))
    }

    #[pyo3(signature = (*_args, **_kwargs))]
    fn __init__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        _args: Bound<'_, PyTuple>,
        _kwargs: Option<Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        let cond_py = slf.unbind();
        Condition::setup_condition(&cond_py, py)
    }

    #[staticmethod]
    fn all_events(events: Bound<'_, PyAny>, count: usize) -> PyResult<bool> {
        let len = events.len()?;
        Ok(len == count)
    }

    #[staticmethod]
    fn any_events(events: Bound<'_, PyAny>, count: usize) -> PyResult<bool> {
        let len = events.len()?;
        Ok(count > 0 || len == 0)
    }
}

/// Condition event that triggers when all of its child events have been triggered.
#[pyclass(extends = Condition, module = "simpy_rs._simpy_rs")]
pub struct AllOf;

impl AllOf {
    pub fn create_rs(
        py: Python<'_>,
        env: Py<Environment>,
        events: Vec<Py<Event>>,
    ) -> PyResult<Py<AllOf>> {
        let all_of = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env))
                .add_subclass(Condition {
                    eval: ConditionEval::All,
                    events,
                    inner: Mutex::new(ConditionInner { count: 0 }),
                })
                .add_subclass(AllOf),
        )?;
        let cond_py: Py<Condition> = all_of.bind(py).as_super().clone().unbind();
        Condition::setup_condition(&cond_py, py)?;
        Ok(all_of)
    }
}

#[pymethods]
impl AllOf {
    #[new]
    fn py_new(
        py: Python<'_>,
        env: Py<Environment>,
        events: Bound<'_, PyAny>,
    ) -> PyResult<PyClassInitializer<Self>> {
        let evs = extract_event_vec(py, events)?;
        Ok(PyClassInitializer::from(Event::new_pending(env))
            .add_subclass(Condition {
                eval: ConditionEval::All,
                events: evs,
                inner: Mutex::new(ConditionInner { count: 0 }),
            })
            .add_subclass(AllOf))
    }

    #[pyo3(signature = (*_args, **_kwargs))]
    fn __init__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        _args: Bound<'_, PyTuple>,
        _kwargs: Option<Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        let cond_py: Py<Condition> = slf.as_super().clone().unbind();
        Condition::setup_condition(&cond_py, py)
    }
}

/// Condition event that triggers when any of its child events has been triggered.
#[pyclass(extends = Condition, module = "simpy_rs._simpy_rs")]
pub struct AnyOf;

impl AnyOf {
    pub fn create_rs(
        py: Python<'_>,
        env: Py<Environment>,
        events: Vec<Py<Event>>,
    ) -> PyResult<Py<AnyOf>> {
        let any_of = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env))
                .add_subclass(Condition {
                    eval: ConditionEval::Any,
                    events,
                    inner: Mutex::new(ConditionInner { count: 0 }),
                })
                .add_subclass(AnyOf),
        )?;
        let cond_py: Py<Condition> = any_of.bind(py).as_super().clone().unbind();
        Condition::setup_condition(&cond_py, py)?;
        Ok(any_of)
    }
}

#[pymethods]
impl AnyOf {
    #[new]
    fn py_new(
        py: Python<'_>,
        env: Py<Environment>,
        events: Bound<'_, PyAny>,
    ) -> PyResult<PyClassInitializer<Self>> {
        let evs = extract_event_vec(py, events)?;
        Ok(PyClassInitializer::from(Event::new_pending(env))
            .add_subclass(Condition {
                eval: ConditionEval::Any,
                events: evs,
                inner: Mutex::new(ConditionInner { count: 0 }),
            })
            .add_subclass(AnyOf))
    }

    #[pyo3(signature = (*_args, **_kwargs))]
    fn __init__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        _args: Bound<'_, PyTuple>,
        _kwargs: Option<Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        let cond_py: Py<Condition> = slf.as_super().clone().unbind();
        Condition::setup_condition(&cond_py, py)
    }
}
