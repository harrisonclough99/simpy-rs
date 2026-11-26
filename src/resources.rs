use std::collections::VecDeque;
use std::sync::Mutex;

use pyo3::basic::CompareOp;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyList;

use crate::env::Environment;
use crate::event::{Event, InternalCallback, Interruption};
use crate::process::Process;

// ============================================================================
// Resource, Request, Release
// ============================================================================

#[pyclass(extends = Event, subclass, module = "simpy_rs._simpy_rs")]
pub struct Request {
    pub resource: Py<PyAny>,
    pub proc: Option<Py<Process>>,
}

impl Request {
    fn cancel_internal(slf: &Bound<'_, Self>, py: Python<'_>) {
        if slf.as_super().borrow().is_triggered() {
            return;
        }
        let res = slf.borrow().resource.clone_ref(py);
        let req_py: Py<Request> = slf.clone().unbind();
        if let Ok(r) = res.bind(py).cast::<Resource>() {
            r.borrow().remove_from_put_queue(&req_py);
        }
    }
}

#[pymethods]
impl Request {
    #[getter]
    fn resource(&self, py: Python<'_>) -> Py<PyAny> {
        self.resource.clone_ref(py)
    }

    #[getter]
    fn proc(&self, py: Python<'_>) -> Option<Py<Process>> {
        self.proc.as_ref().map(|p| p.clone_ref(py))
    }

    fn cancel(slf: Bound<'_, Self>, py: Python<'_>) {
        Self::cancel_internal(&slf, py);
    }

    fn __enter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    #[pyo3(signature = (_exc_type = None, _exc_val = None, _exc_tb = None))]
    fn __exit__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        Self::cancel_internal(&slf, py);
        let res = slf.borrow().resource.clone_ref(py);
        if let Ok(r) = res.bind(py).cast::<Resource>() {
            Resource::release_rs(&r.clone().unbind(), py, slf.clone().unbind())?;
        }
        Ok(false)
    }
}

#[pyclass(extends = Event, subclass, module = "simpy_rs._simpy_rs")]
pub struct Release {
    pub resource: Py<PyAny>,
    pub request: Py<PyAny>,
}

#[pymethods]
impl Release {
    #[getter]
    fn resource(&self, py: Python<'_>) -> Py<PyAny> {
        self.resource.clone_ref(py)
    }

    #[getter]
    fn request(&self, py: Python<'_>) -> Py<PyAny> {
        self.request.clone_ref(py)
    }

    fn cancel(&self) {}

    fn __enter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    #[pyo3(signature = (_exc_type = None, _exc_val = None, _exc_tb = None))]
    fn __exit__(
        &self,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> bool {
        false
    }
}

struct ResourceInner {
    users: Vec<Py<Request>>,
    put_queue: VecDeque<Py<Request>>,
    get_queue: VecDeque<Py<Release>>,
}

/// Resource with `capacity` usage slots that can be requested by processes.
#[pyclass(subclass, module = "simpy_rs._simpy_rs")]
pub struct Resource {
    pub env: Py<Environment>,
    pub capacity: usize,
    inner: Mutex<ResourceInner>,
}

impl Resource {
    pub fn remove_from_put_queue(&self, req: &Py<Request>) {
        let mut inner = self.inner.lock().unwrap();
        inner.put_queue.retain(|r| !r.is(req));
    }

    pub fn trigger_put_rs(res_py: &Py<Resource>, py: Python<'_>) -> PyResult<()> {
        loop {
            let next_req = {
                let res = res_py.borrow(py);
                let mut inner = res.inner.lock().unwrap();
                if inner.users.len() < res.capacity {
                    if let Some(req) = inner.put_queue.pop_front() {
                        inner.users.push(req.clone_ref(py));
                        Some(req)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            match next_req {
                Some(req) => {
                    let ev_py: Py<Event> = req.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, py.None())?;
                }
                None => break,
            }
        }
        Ok(())
    }

    pub fn trigger_get_rs(res_py: &Py<Resource>, py: Python<'_>) -> PyResult<()> {
        loop {
            let next_rel = {
                let res = res_py.borrow(py);
                let mut inner = res.inner.lock().unwrap();
                if let Some(rel) = inner.get_queue.pop_front() {
                    let req_ref = rel.borrow(py).request.clone_ref(py);
                    if let Some(pos) = inner
                        .users
                        .iter()
                        .position(|u| u.as_ptr() == req_ref.as_ptr())
                    {
                        inner.users.remove(pos);
                    }
                    Some(rel)
                } else {
                    None
                }
            };
            match next_rel {
                Some(rel) => {
                    let ev_py: Py<Event> = rel.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, py.None())?;
                }
                None => break,
            }
        }
        Ok(())
    }

    pub fn release_rs(
        res_py: &Py<Resource>,
        py: Python<'_>,
        request: Py<Request>,
    ) -> PyResult<Py<Release>> {
        let env = res_py.borrow(py).env.clone_ref(py);
        let rel = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(Release {
                resource: res_py.clone_ref(py).into_any(),
                request: request.into_any(),
            }),
        )?;
        rel.bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::ResourcePut(res_py.clone_ref(py)));
        {
            let res = res_py.borrow(py);
            res.inner.lock().unwrap().get_queue.push_back(rel.clone_ref(py));
        }
        Self::trigger_get_rs(res_py, py)?;
        Ok(rel)
    }
}

#[pymethods]
impl Resource {
    #[new]
    #[pyo3(signature = (env, capacity = 1))]
    fn py_new(env: Py<Environment>, capacity: isize) -> PyResult<Self> {
        if capacity <= 0 {
            return Err(PyValueError::new_err("\"capacity\" must be > 0."));
        }
        Ok(Self {
            env,
            capacity: capacity as usize,
            inner: Mutex::new(ResourceInner {
                users: Vec::new(),
                put_queue: VecDeque::new(),
                get_queue: VecDeque::new(),
            }),
        })
    }

    #[getter]
    fn capacity(&self) -> usize {
        self.capacity
    }

    #[getter]
    fn count(&self) -> usize {
        self.inner.lock().unwrap().users.len()
    }

    #[getter]
    fn users(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, Request>> = inner.users.iter().map(|u| u.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, Request>> = inner.put_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn put_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        self.queue(py)
    }

    #[getter]
    fn get_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, Release>> = inner.get_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    fn request(slf: Bound<'_, Self>, py: Python<'_>) -> PyResult<Py<Request>> {
        let env = slf.borrow().env.clone_ref(py);
        let proc = env.borrow(py).active_process_rs(py);
        let res_py = slf.clone().unbind();
        let req = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(Request {
                resource: res_py.clone_ref(py).into_any(),
                proc,
            }),
        )?;
        req.bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::ResourceGet(res_py.clone_ref(py)));
        {
            let res = slf.borrow();
            res.inner.lock().unwrap().put_queue.push_back(req.clone_ref(py));
        }
        Self::trigger_put_rs(&res_py, py)?;
        Ok(req)
    }

    fn release(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        request: Py<Request>,
    ) -> PyResult<Py<Release>> {
        Self::release_rs(&slf.unbind(), py, request)
    }
}

// ============================================================================
// PriorityRequest, Preempted, PriorityResource, PreemptiveResource
// ============================================================================

/// Cause object passed to `Interrupt` when a process is preempted from a `PreemptiveResource`.
#[pyclass(module = "simpy_rs._simpy_rs")]
pub struct Preempted {
    pub by: Option<Py<Process>>,
    pub usage_since: Option<f64>,
    pub resource: Py<PyAny>,
}

#[pymethods]
impl Preempted {
    #[new]
    fn py_new(by: Option<Py<Process>>, usage_since: Option<f64>, resource: Py<PyAny>) -> Self {
        Self {
            by,
            usage_since,
            resource,
        }
    }

    #[getter]
    fn by(&self, py: Python<'_>) -> Option<Py<Process>> {
        self.by.as_ref().map(|p| p.clone_ref(py))
    }

    #[getter]
    fn usage_since(&self) -> Option<f64> {
        self.usage_since
    }

    #[getter]
    fn resource(&self, py: Python<'_>) -> Py<PyAny> {
        self.resource.clone_ref(py)
    }

    fn __repr__(&self) -> String {
        format!(
            "Preempted(usage_since={:?})",
            self.usage_since
        )
    }
}

struct PriorityRequestInner {
    usage_since: Option<f64>,
}

#[pyclass(extends = Event, subclass, module = "simpy_rs._simpy_rs")]
pub struct PriorityRequest {
    pub resource: Py<PyAny>,
    pub proc: Option<Py<Process>>,
    pub priority: f64,
    pub preempt: bool,
    pub time: f64,
    pub seq: u64,
    inner: Mutex<PriorityRequestInner>,
}

impl PriorityRequest {
    pub fn sort_key(&self) -> (f64, f64, bool, u64) {
        (self.priority, self.time, !self.preempt, self.seq)
    }

    fn cmp_key(a: &(f64, f64, bool, u64), b: &(f64, f64, bool, u64)) -> std::cmp::Ordering {
        a.0.total_cmp(&b.0)
            .then_with(|| a.1.total_cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.3.cmp(&b.3))
    }

    fn preempt_cmp_key(a: &(f64, f64, bool), b: &(f64, f64, bool)) -> std::cmp::Ordering {
        a.0.total_cmp(&b.0)
            .then_with(|| a.1.total_cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    }

    fn cancel_internal(slf: &Bound<'_, Self>, py: Python<'_>) {
        if slf.as_super().borrow().is_triggered() {
            return;
        }
        let res = slf.borrow().resource.clone_ref(py);
        let req_py: Py<PriorityRequest> = slf.clone().unbind();
        if let Ok(r) = res.bind(py).cast::<PriorityResource>() {
            r.borrow().remove_from_put_queue(&req_py);
        } else if let Ok(r) = res.bind(py).cast::<PreemptiveResource>() {
            r.borrow().remove_from_put_queue(&req_py);
        }
    }
}

#[pymethods]
impl PriorityRequest {
    #[getter]
    fn resource(&self, py: Python<'_>) -> Py<PyAny> {
        self.resource.clone_ref(py)
    }

    #[getter]
    fn proc(&self, py: Python<'_>) -> Option<Py<Process>> {
        self.proc.as_ref().map(|p| p.clone_ref(py))
    }

    #[getter]
    fn priority(&self) -> f64 {
        self.priority
    }

    #[getter]
    fn preempt(&self) -> bool {
        self.preempt
    }

    #[getter]
    fn time(&self) -> f64 {
        self.time
    }

    #[getter]
    fn usage_since(&self) -> Option<f64> {
        self.inner.lock().unwrap().usage_since
    }

    #[getter]
    fn key(&self) -> (f64, f64, bool) {
        (self.priority, self.time, !self.preempt)
    }

    fn cancel(slf: Bound<'_, Self>, py: Python<'_>) {
        Self::cancel_internal(&slf, py);
    }

    fn __enter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    #[pyo3(signature = (_exc_type = None, _exc_val = None, _exc_tb = None))]
    fn __exit__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        Self::cancel_internal(&slf, py);
        let res = slf.borrow().resource.clone_ref(py);
        if let Ok(r) = res.bind(py).cast::<PriorityResource>() {
            PriorityResource::release_rs(&r.clone().unbind(), py, slf.clone().unbind())?;
        } else if let Ok(r) = res.bind(py).cast::<PreemptiveResource>() {
            PreemptiveResource::release_rs(&r.clone().unbind(), py, slf.clone().unbind())?;
        }
        Ok(false)
    }
}

struct PriorityResourceInner {
    users: Vec<Py<PriorityRequest>>,
    put_queue: Vec<Py<PriorityRequest>>,
    get_queue: VecDeque<Py<Release>>,
}

/// Resource supporting prioritized requests (lower priority value = higher priority).
#[pyclass(subclass, module = "simpy_rs._simpy_rs")]
pub struct PriorityResource {
    pub env: Py<Environment>,
    pub capacity: usize,
    inner: Mutex<PriorityResourceInner>,
}

impl PriorityResource {
    pub fn remove_from_put_queue(&self, req: &Py<PriorityRequest>) {
        let mut inner = self.inner.lock().unwrap();
        inner.put_queue.retain(|r| !r.is(req));
    }

    pub fn trigger_put_rs(res_py: &Py<PriorityResource>, py: Python<'_>) -> PyResult<()> {
        loop {
            let next_req = {
                let res = res_py.borrow(py);
                let mut inner = res.inner.lock().unwrap();
                if inner.users.len() < res.capacity && !inner.put_queue.is_empty() {
                    let req = inner.put_queue.remove(0);
                    let now = res.env.borrow(py).now_rs();
                    req.borrow(py).inner.lock().unwrap().usage_since = Some(now);
                    inner.users.push(req.clone_ref(py));
                    Some(req)
                } else {
                    None
                }
            };
            match next_req {
                Some(req) => {
                    let ev_py: Py<Event> = req.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, py.None())?;
                }
                None => break,
            }
        }
        Ok(())
    }

    pub fn trigger_get_rs(res_py: &Py<PriorityResource>, py: Python<'_>) -> PyResult<()> {
        loop {
            let next_rel = {
                let res = res_py.borrow(py);
                let mut inner = res.inner.lock().unwrap();
                if let Some(rel) = inner.get_queue.pop_front() {
                    let req_ref = rel.borrow(py).request.clone_ref(py);
                    if let Some(pos) = inner
                        .users
                        .iter()
                        .position(|u| u.as_ptr() == req_ref.as_ptr())
                    {
                        inner.users.remove(pos);
                    }
                    Some(rel)
                } else {
                    None
                }
            };
            match next_rel {
                Some(rel) => {
                    let ev_py: Py<Event> = rel.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, py.None())?;
                }
                None => break,
            }
        }
        Ok(())
    }

    pub fn release_rs(
        res_py: &Py<PriorityResource>,
        py: Python<'_>,
        request: Py<PriorityRequest>,
    ) -> PyResult<Py<Release>> {
        let env = res_py.borrow(py).env.clone_ref(py);
        let rel = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(Release {
                resource: res_py.clone_ref(py).into_any(),
                request: request.into_any(),
            }),
        )?;
        rel.bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::PriorityResourcePut(res_py.clone_ref(py)));
        {
            let res = res_py.borrow(py);
            res.inner.lock().unwrap().get_queue.push_back(rel.clone_ref(py));
        }
        Self::trigger_get_rs(res_py, py)?;
        Ok(rel)
    }
}

#[pymethods]
impl PriorityResource {
    #[new]
    #[pyo3(signature = (env, capacity = 1))]
    fn py_new(env: Py<Environment>, capacity: isize) -> PyResult<Self> {
        if capacity <= 0 {
            return Err(PyValueError::new_err("\"capacity\" must be > 0."));
        }
        Ok(Self {
            env,
            capacity: capacity as usize,
            inner: Mutex::new(PriorityResourceInner {
                users: Vec::new(),
                put_queue: Vec::new(),
                get_queue: VecDeque::new(),
            }),
        })
    }

    #[getter]
    fn capacity(&self) -> usize {
        self.capacity
    }

    #[getter]
    fn count(&self) -> usize {
        self.inner.lock().unwrap().users.len()
    }

    #[getter]
    fn users(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, PriorityRequest>> =
            inner.users.iter().map(|u| u.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, PriorityRequest>> =
            inner.put_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn put_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        self.queue(py)
    }

    #[getter]
    fn get_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, Release>> = inner.get_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[pyo3(signature = (priority = 0.0, preempt = true))]
    fn request(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        priority: f64,
        preempt: bool,
    ) -> PyResult<Py<PriorityRequest>> {
        let env = slf.borrow().env.clone_ref(py);
        let (now, seq, proc) = {
            let e = env.borrow(py);
            (e.now_rs(), e.next_eid(), e.active_process_rs(py))
        };
        let res_py = slf.clone().unbind();
        let req = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(PriorityRequest {
                resource: res_py.clone_ref(py).into_any(),
                proc,
                priority,
                preempt,
                time: now,
                seq,
                inner: Mutex::new(PriorityRequestInner { usage_since: None }),
            }),
        )?;
        req.bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::PriorityResourceGet(res_py.clone_ref(py)));
        {
            let res = slf.borrow();
            let mut inner = res.inner.lock().unwrap();
            let new_key = req.borrow(py).sort_key();
            let pos = inner
                .put_queue
                .iter()
                .position(|existing| {
                    let ek = existing.borrow(py).sort_key();
                    PriorityRequest::cmp_key(&new_key, &ek) == std::cmp::Ordering::Less
                })
                .unwrap_or(inner.put_queue.len());
            inner.put_queue.insert(pos, req.clone_ref(py));
        }
        Self::trigger_put_rs(&res_py, py)?;
        Ok(req)
    }

    fn release(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        request: Py<PriorityRequest>,
    ) -> PyResult<Py<Release>> {
        Self::release_rs(&slf.unbind(), py, request)
    }
}

/// PriorityResource that allows higher-priority requests to preempt lower-priority users.
#[pyclass(subclass, module = "simpy_rs._simpy_rs")]
pub struct PreemptiveResource {
    pub env: Py<Environment>,
    pub capacity: usize,
    inner: Mutex<PriorityResourceInner>,
}

impl PreemptiveResource {
    pub fn remove_from_put_queue(&self, req: &Py<PriorityRequest>) {
        let mut inner = self.inner.lock().unwrap();
        inner.put_queue.retain(|r| !r.is(req));
    }

    pub fn trigger_put_rs(res_py: &Py<PreemptiveResource>, py: Python<'_>) -> PyResult<()> {
        loop {
            enum Action {
                Grant(Py<PriorityRequest>),
                PreemptAndGrant {
                    victim_proc: Option<Py<Process>>,
                    victim_usage_since: Option<f64>,
                    by_proc: Option<Py<Process>>,
                    req: Py<PriorityRequest>,
                },
                Stop,
            }

            let action = {
                let res = res_py.borrow(py);
                let mut inner = res.inner.lock().unwrap();
                if inner.put_queue.is_empty() {
                    Action::Stop
                } else if inner.users.len() < res.capacity {
                    let req = inner.put_queue.remove(0);
                    let now = res.env.borrow(py).now_rs();
                    req.borrow(py).inner.lock().unwrap().usage_since = Some(now);
                    inner.users.push(req.clone_ref(py));
                    Action::Grant(req)
                } else {
                    // Check if the highest-priority waiting request can preempt the lowest-priority user.
                    let candidate = &inner.put_queue[0];
                    let (cand_preempt, cand_key, cand_proc) = {
                        let c = candidate.borrow(py);
                        (
                            c.preempt,
                            (c.priority, c.time, !c.preempt),
                            c.proc.as_ref().map(|p| p.clone_ref(py)),
                        )
                    };
                    if cand_preempt && !inner.users.is_empty() {
                        // Find user with maximum (priority, time, !preempt)
                        let mut worst_idx = 0;
                        let mut worst_key = {
                            let u = inner.users[0].borrow(py);
                            (u.priority, u.time, !u.preempt)
                        };
                        for (i, u_py) in inner.users.iter().enumerate().skip(1) {
                            let u = u_py.borrow(py);
                            let uk = (u.priority, u.time, !u.preempt);
                            if PriorityRequest::preempt_cmp_key(&uk, &worst_key)
                                == std::cmp::Ordering::Greater
                            {
                                worst_idx = i;
                                worst_key = uk;
                            }
                        }
                        if PriorityRequest::preempt_cmp_key(&worst_key, &cand_key)
                            == std::cmp::Ordering::Greater
                        {
                            let victim = inner.users.remove(worst_idx);
                            let victim_proc =
                                victim.borrow(py).proc.as_ref().map(|p| p.clone_ref(py));
                            let victim_usage_since =
                                victim.borrow(py).inner.lock().unwrap().usage_since;
                            let req = inner.put_queue.remove(0);
                            let now = res.env.borrow(py).now_rs();
                            req.borrow(py).inner.lock().unwrap().usage_since = Some(now);
                            inner.users.push(req.clone_ref(py));
                            Action::PreemptAndGrant {
                                victim_proc,
                                victim_usage_since,
                                by_proc: cand_proc,
                                req,
                            }
                        } else {
                            Action::Stop
                        }
                    } else {
                        Action::Stop
                    }
                }
            };

            match action {
                Action::Grant(req) => {
                    let ev_py: Py<Event> = req.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, py.None())?;
                }
                Action::PreemptAndGrant {
                    victim_proc,
                    victim_usage_since,
                    by_proc,
                    req,
                } => {
                    if let Some(vp) = victim_proc {
                        if !vp.bind(py).as_super().borrow().is_triggered() {
                            let env = res_py.borrow(py).env.clone_ref(py);
                            let cause = Py::new(
                                py,
                                Preempted {
                                    by: by_proc,
                                    usage_since: victim_usage_since,
                                    resource: res_py.clone_ref(py).into_any(),
                                },
                            )?
                            .into_any();
                            Interruption::create_and_schedule(py, env, vp, cause)?;
                        }
                    }
                    let ev_py: Py<Event> = req.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, py.None())?;
                }
                Action::Stop => break,
            }
        }
        Ok(())
    }

    pub fn trigger_get_rs(res_py: &Py<PreemptiveResource>, py: Python<'_>) -> PyResult<()> {
        loop {
            let next_rel = {
                let res = res_py.borrow(py);
                let mut inner = res.inner.lock().unwrap();
                if let Some(rel) = inner.get_queue.pop_front() {
                    let req_ref = rel.borrow(py).request.clone_ref(py);
                    if let Some(pos) = inner
                        .users
                        .iter()
                        .position(|u| u.as_ptr() == req_ref.as_ptr())
                    {
                        inner.users.remove(pos);
                    }
                    Some(rel)
                } else {
                    None
                }
            };
            match next_rel {
                Some(rel) => {
                    let ev_py: Py<Event> = rel.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, py.None())?;
                }
                None => break,
            }
        }
        Ok(())
    }

    pub fn release_rs(
        res_py: &Py<PreemptiveResource>,
        py: Python<'_>,
        request: Py<PriorityRequest>,
    ) -> PyResult<Py<Release>> {
        let env = res_py.borrow(py).env.clone_ref(py);
        let rel = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(Release {
                resource: res_py.clone_ref(py).into_any(),
                request: request.into_any(),
            }),
        )?;
        rel.bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::PreemptiveResourcePut(
                res_py.clone_ref(py),
            ));
        {
            let res = res_py.borrow(py);
            res.inner.lock().unwrap().get_queue.push_back(rel.clone_ref(py));
        }
        Self::trigger_get_rs(res_py, py)?;
        Ok(rel)
    }
}

#[pymethods]
impl PreemptiveResource {
    #[new]
    #[pyo3(signature = (env, capacity = 1))]
    fn py_new(env: Py<Environment>, capacity: isize) -> PyResult<Self> {
        if capacity <= 0 {
            return Err(PyValueError::new_err("\"capacity\" must be > 0."));
        }
        Ok(Self {
            env,
            capacity: capacity as usize,
            inner: Mutex::new(PriorityResourceInner {
                users: Vec::new(),
                put_queue: Vec::new(),
                get_queue: VecDeque::new(),
            }),
        })
    }

    #[getter]
    fn capacity(&self) -> usize {
        self.capacity
    }

    #[getter]
    fn count(&self) -> usize {
        self.inner.lock().unwrap().users.len()
    }

    #[getter]
    fn users(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, PriorityRequest>> =
            inner.users.iter().map(|u| u.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, PriorityRequest>> =
            inner.put_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn put_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        self.queue(py)
    }

    #[getter]
    fn get_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, Release>> = inner.get_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[pyo3(signature = (priority = 0.0, preempt = true))]
    fn request(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        priority: f64,
        preempt: bool,
    ) -> PyResult<Py<PriorityRequest>> {
        let env = slf.borrow().env.clone_ref(py);
        let (now, seq, proc) = {
            let e = env.borrow(py);
            (e.now_rs(), e.next_eid(), e.active_process_rs(py))
        };
        let res_py = slf.clone().unbind();
        let req = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(PriorityRequest {
                resource: res_py.clone_ref(py).into_any(),
                proc,
                priority,
                preempt,
                time: now,
                seq,
                inner: Mutex::new(PriorityRequestInner { usage_since: None }),
            }),
        )?;
        req.bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::PreemptiveResourceGet(
                res_py.clone_ref(py),
            ));
        {
            let res = slf.borrow();
            let mut inner = res.inner.lock().unwrap();
            let new_key = req.borrow(py).sort_key();
            let pos = inner
                .put_queue
                .iter()
                .position(|existing| {
                    let ek = existing.borrow(py).sort_key();
                    PriorityRequest::cmp_key(&new_key, &ek) == std::cmp::Ordering::Less
                })
                .unwrap_or(inner.put_queue.len());
            inner.put_queue.insert(pos, req.clone_ref(py));
        }
        Self::trigger_put_rs(&res_py, py)?;
        Ok(req)
    }

    fn release(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        request: Py<PriorityRequest>,
    ) -> PyResult<Py<Release>> {
        Self::release_rs(&slf.unbind(), py, request)
    }
}

// ============================================================================
// Container, ContainerPut, ContainerGet
// ============================================================================

#[pyclass(extends = Event, subclass, module = "simpy_rs._simpy_rs")]
pub struct ContainerPut {
    pub resource: Py<Container>,
    pub amount: f64,
    pub proc: Option<Py<Process>>,
}

#[pymethods]
impl ContainerPut {
    #[getter]
    fn amount(&self) -> f64 {
        self.amount
    }

    #[getter]
    fn proc(&self, py: Python<'_>) -> Option<Py<Process>> {
        self.proc.as_ref().map(|p| p.clone_ref(py))
    }

    fn cancel(slf: Bound<'_, Self>, py: Python<'_>) {
        if !slf.as_super().borrow().is_triggered() {
            let cont = slf.borrow().resource.clone_ref(py);
            let me = slf.unbind();
            cont.borrow(py).inner.lock().unwrap().put_queue.retain(|p| !p.is(&me));
        }
    }

    fn __enter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    #[pyo3(signature = (_exc_type = None, _exc_val = None, _exc_tb = None))]
    fn __exit__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> bool {
        Self::cancel(slf, py);
        false
    }
}

#[pyclass(extends = Event, subclass, module = "simpy_rs._simpy_rs")]
pub struct ContainerGet {
    pub resource: Py<Container>,
    pub amount: f64,
    pub proc: Option<Py<Process>>,
}

#[pymethods]
impl ContainerGet {
    #[getter]
    fn amount(&self) -> f64 {
        self.amount
    }

    #[getter]
    fn proc(&self, py: Python<'_>) -> Option<Py<Process>> {
        self.proc.as_ref().map(|p| p.clone_ref(py))
    }

    fn cancel(slf: Bound<'_, Self>, py: Python<'_>) {
        if !slf.as_super().borrow().is_triggered() {
            let cont = slf.borrow().resource.clone_ref(py);
            let me = slf.unbind();
            cont.borrow(py).inner.lock().unwrap().get_queue.retain(|g| !g.is(&me));
        }
    }

    fn __enter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    #[pyo3(signature = (_exc_type = None, _exc_val = None, _exc_tb = None))]
    fn __exit__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> bool {
        Self::cancel(slf, py);
        false
    }
}

struct ContainerInner {
    level: f64,
    put_queue: VecDeque<Py<ContainerPut>>,
    get_queue: VecDeque<Py<ContainerGet>>,
}

/// Resource containing up to `capacity` of matter which may either be continuous or discrete.
#[pyclass(subclass, module = "simpy_rs._simpy_rs")]
pub struct Container {
    pub env: Py<Environment>,
    pub capacity: f64,
    inner: Mutex<ContainerInner>,
}

impl Container {
    pub fn trigger_put_rs(cont_py: &Py<Container>, py: Python<'_>) -> PyResult<()> {
        let mut idx = 0;
        loop {
            let next_put = {
                let cont = cont_py.borrow(py);
                let mut inner = cont.inner.lock().unwrap();
                if idx >= inner.put_queue.len() {
                    None
                } else {
                    let amount = inner.put_queue[idx].borrow(py).amount;
                    if cont.capacity - inner.level >= amount {
                        inner.level += amount;
                        Some(inner.put_queue.remove(idx).unwrap())
                    } else {
                        // In SimPy Container, a blocked put stops further puts in the queue.
                        None
                    }
                }
            };
            match next_put {
                Some(put_ev) => {
                    let ev_py: Py<Event> = put_ev.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, py.None())?;
                }
                None => {
                    let _ = idx;
                    break;
                }
            }
            idx = 0;
        }
        Ok(())
    }

    pub fn trigger_get_rs(cont_py: &Py<Container>, py: Python<'_>) -> PyResult<()> {
        loop {
            let next_get = {
                let cont = cont_py.borrow(py);
                let mut inner = cont.inner.lock().unwrap();
                if let Some(front) = inner.get_queue.front() {
                    let amount = front.borrow(py).amount;
                    if inner.level >= amount {
                        inner.level -= amount;
                        inner.get_queue.pop_front()
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            match next_get {
                Some(get_ev) => {
                    let ev_py: Py<Event> = get_ev.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, py.None())?;
                }
                None => break,
            }
        }
        Ok(())
    }
}

#[pymethods]
impl Container {
    #[new]
    #[pyo3(signature = (env, capacity = f64::INFINITY, init = 0.0))]
    fn py_new(env: Py<Environment>, capacity: f64, init: f64) -> PyResult<Self> {
        if capacity.is_nan() || capacity <= 0.0 {
            return Err(PyValueError::new_err("\"capacity\" must be > 0."));
        }
        if init.is_nan() || init < 0.0 {
            return Err(PyValueError::new_err("\"init\" must be >= 0."));
        }
        if init > capacity {
            return Err(PyValueError::new_err("\"init\" must be <= \"capacity\"."));
        }
        Ok(Self {
            env,
            capacity,
            inner: Mutex::new(ContainerInner {
                level: init,
                put_queue: VecDeque::new(),
                get_queue: VecDeque::new(),
            }),
        })
    }

    #[getter]
    fn capacity(&self) -> f64 {
        self.capacity
    }

    #[getter]
    fn level(&self) -> f64 {
        self.inner.lock().unwrap().level
    }

    #[getter]
    fn put_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, ContainerPut>> =
            inner.put_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn get_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, ContainerGet>> =
            inner.get_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    fn put(slf: Bound<'_, Self>, py: Python<'_>, amount: f64) -> PyResult<Py<ContainerPut>> {
        if amount.is_nan() || amount <= 0.0 {
            return Err(PyValueError::new_err(format!("amount(={amount}) must be > 0.")));
        }
        let env = slf.borrow().env.clone_ref(py);
        let proc = env.borrow(py).active_process_rs(py);
        let cont_py = slf.clone().unbind();
        let put_ev = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(ContainerPut {
                resource: cont_py.clone_ref(py),
                amount,
                proc,
            }),
        )?;
        put_ev
            .bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::ContainerGet(cont_py.clone_ref(py)));
        {
            let cont = slf.borrow();
            cont.inner.lock().unwrap().put_queue.push_back(put_ev.clone_ref(py));
        }
        Self::trigger_put_rs(&cont_py, py)?;
        Ok(put_ev)
    }

    fn get(slf: Bound<'_, Self>, py: Python<'_>, amount: f64) -> PyResult<Py<ContainerGet>> {
        if amount.is_nan() || amount <= 0.0 {
            return Err(PyValueError::new_err(format!("amount(={amount}) must be > 0.")));
        }
        let env = slf.borrow().env.clone_ref(py);
        let proc = env.borrow(py).active_process_rs(py);
        let cont_py = slf.clone().unbind();
        let get_ev = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(ContainerGet {
                resource: cont_py.clone_ref(py),
                amount,
                proc,
            }),
        )?;
        get_ev
            .bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::ContainerPut(cont_py.clone_ref(py)));
        {
            let cont = slf.borrow();
            cont.inner.lock().unwrap().get_queue.push_back(get_ev.clone_ref(py));
        }
        Self::trigger_get_rs(&cont_py, py)?;
        Ok(get_ev)
    }
}

// ============================================================================
// Store, StorePut, StoreGet, PriorityItem, PriorityStore, FilterStore
// ============================================================================

#[pyclass(extends = Event, subclass, module = "simpy_rs._simpy_rs")]
pub struct StorePut {
    pub resource: Py<PyAny>,
    pub item: Py<PyAny>,
    pub proc: Option<Py<Process>>,
}

#[pymethods]
impl StorePut {
    #[getter]
    fn item(&self, py: Python<'_>) -> Py<PyAny> {
        self.item.clone_ref(py)
    }

    #[getter]
    fn proc(&self, py: Python<'_>) -> Option<Py<Process>> {
        self.proc.as_ref().map(|p| p.clone_ref(py))
    }

    fn cancel(slf: Bound<'_, Self>, py: Python<'_>) {
        if slf.as_super().borrow().is_triggered() {
            return;
        }
        let res = slf.borrow().resource.clone_ref(py);
        let me = slf.unbind();
        if let Ok(s) = res.bind(py).cast::<Store>() {
            s.borrow().inner.lock().unwrap().put_queue.retain(|p| !p.is(&me));
        } else if let Ok(s) = res.bind(py).cast::<PriorityStore>() {
            s.borrow().inner.lock().unwrap().put_queue.retain(|p| !p.is(&me));
        } else if let Ok(s) = res.bind(py).cast::<FilterStore>() {
            s.borrow().inner.lock().unwrap().put_queue.retain(|p| !p.is(&me));
        }
    }

    fn __enter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    #[pyo3(signature = (_exc_type = None, _exc_val = None, _exc_tb = None))]
    fn __exit__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> bool {
        Self::cancel(slf, py);
        false
    }
}

#[pyclass(extends = Event, subclass, module = "simpy_rs._simpy_rs")]
pub struct StoreGet {
    pub resource: Py<PyAny>,
    pub proc: Option<Py<Process>>,
}

#[pymethods]
impl StoreGet {
    #[getter]
    fn proc(&self, py: Python<'_>) -> Option<Py<Process>> {
        self.proc.as_ref().map(|p| p.clone_ref(py))
    }

    fn cancel(slf: Bound<'_, Self>, py: Python<'_>) {
        if slf.as_super().borrow().is_triggered() {
            return;
        }
        let res = slf.borrow().resource.clone_ref(py);
        let me = slf.unbind();
        if let Ok(s) = res.bind(py).cast::<Store>() {
            s.borrow().inner.lock().unwrap().get_queue.retain(|g| !g.is(&me));
        } else if let Ok(s) = res.bind(py).cast::<PriorityStore>() {
            s.borrow().inner.lock().unwrap().get_queue.retain(|g| !g.is(&me));
        }
    }

    fn __enter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    #[pyo3(signature = (_exc_type = None, _exc_val = None, _exc_tb = None))]
    fn __exit__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> bool {
        Self::cancel(slf, py);
        false
    }
}

struct StoreInner {
    items: VecDeque<Py<PyAny>>,
    put_queue: VecDeque<Py<StorePut>>,
    get_queue: VecDeque<Py<StoreGet>>,
}

/// Resource with `capacity` slots for storing arbitrary Python objects in FIFO order.
#[pyclass(subclass, module = "simpy_rs._simpy_rs")]
pub struct Store {
    pub env: Py<Environment>,
    pub capacity: f64,
    inner: Mutex<StoreInner>,
}

impl Store {
    pub fn trigger_put_rs(store_py: &Py<Store>, py: Python<'_>) -> PyResult<()> {
        loop {
            let next_put = {
                let s = store_py.borrow(py);
                let mut inner = s.inner.lock().unwrap();
                if (inner.items.len() as f64) < s.capacity {
                    if let Some(put_ev) = inner.put_queue.pop_front() {
                        let item = put_ev.borrow(py).item.clone_ref(py);
                        inner.items.push_back(item);
                        Some(put_ev)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            match next_put {
                Some(put_ev) => {
                    let ev_py: Py<Event> = put_ev.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, py.None())?;
                }
                None => break,
            }
        }
        Ok(())
    }

    pub fn trigger_get_rs(store_py: &Py<Store>, py: Python<'_>) -> PyResult<()> {
        loop {
            let next_get = {
                let s = store_py.borrow(py);
                let mut inner = s.inner.lock().unwrap();
                if !inner.items.is_empty() {
                    if let Some(get_ev) = inner.get_queue.pop_front() {
                        let item = inner.items.pop_front().unwrap();
                        Some((get_ev, item))
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            match next_get {
                Some((get_ev, item)) => {
                    let ev_py: Py<Event> = get_ev.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, item)?;
                }
                None => break,
            }
        }
        Ok(())
    }
}

#[pymethods]
impl Store {
    #[new]
    #[pyo3(signature = (env, capacity = f64::INFINITY))]
    fn py_new(env: Py<Environment>, capacity: f64) -> PyResult<Self> {
        if capacity.is_nan() || capacity <= 0.0 {
            return Err(PyValueError::new_err("\"capacity\" must be > 0."));
        }
        Ok(Self {
            env,
            capacity,
            inner: Mutex::new(StoreInner {
                items: VecDeque::new(),
                put_queue: VecDeque::new(),
                get_queue: VecDeque::new(),
            }),
        })
    }

    #[getter]
    fn capacity(&self) -> f64 {
        self.capacity
    }

    #[getter]
    fn items(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, PyAny>> = inner.items.iter().map(|i| i.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn put_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, StorePut>> = inner.put_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn get_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, StoreGet>> = inner.get_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    fn put(slf: Bound<'_, Self>, py: Python<'_>, item: Py<PyAny>) -> PyResult<Py<StorePut>> {
        let env = slf.borrow().env.clone_ref(py);
        let proc = env.borrow(py).active_process_rs(py);
        let store_py = slf.clone().unbind();
        let put_ev = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(StorePut {
                resource: store_py.clone_ref(py).into_any(),
                item,
                proc,
            }),
        )?;
        put_ev
            .bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::StoreGet(store_py.clone_ref(py)));
        {
            let s = slf.borrow();
            s.inner.lock().unwrap().put_queue.push_back(put_ev.clone_ref(py));
        }
        Self::trigger_put_rs(&store_py, py)?;
        Ok(put_ev)
    }

    fn get(slf: Bound<'_, Self>, py: Python<'_>) -> PyResult<Py<StoreGet>> {
        let env = slf.borrow().env.clone_ref(py);
        let proc = env.borrow(py).active_process_rs(py);
        let store_py = slf.clone().unbind();
        let get_ev = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(StoreGet {
                resource: store_py.clone_ref(py).into_any(),
                proc,
            }),
        )?;
        get_ev
            .bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::StorePut(store_py.clone_ref(py)));
        {
            let s = slf.borrow();
            s.inner.lock().unwrap().get_queue.push_back(get_ev.clone_ref(py));
        }
        Self::trigger_get_rs(&store_py, py)?;
        Ok(get_ev)
    }
}

/// Wrapper for storing uncomparable objects in a `PriorityStore`, comparing only by `priority`.
#[pyclass(module = "simpy_rs._simpy_rs")]
pub struct PriorityItem {
    pub priority: Py<PyAny>,
    pub item: Py<PyAny>,
}

#[pymethods]
impl PriorityItem {
    #[new]
    fn py_new(priority: Py<PyAny>, item: Py<PyAny>) -> Self {
        Self { priority, item }
    }

    #[getter]
    fn priority(&self, py: Python<'_>) -> Py<PyAny> {
        self.priority.clone_ref(py)
    }

    #[getter]
    fn item(&self, py: Python<'_>) -> Py<PyAny> {
        self.item.clone_ref(py)
    }

    fn __richcmp__(
        &self,
        py: Python<'_>,
        other: Bound<'_, PriorityItem>,
        op: CompareOp,
    ) -> PyResult<bool> {
        let a = self.priority.bind(py);
        let b = other.borrow().priority.clone_ref(py);
        let b_bound = b.bind(py);
        match op {
            CompareOp::Lt => a.lt(b_bound),
            CompareOp::Le => a.le(b_bound),
            CompareOp::Eq => a.eq(b_bound),
            CompareOp::Ne => a.ne(b_bound),
            CompareOp::Gt => a.gt(b_bound),
            CompareOp::Ge => a.ge(b_bound),
        }
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let p = self.priority.bind(py).repr()?;
        let i = self.item.bind(py).repr()?;
        Ok(format!("PriorityItem(priority={p}, item={i})"))
    }
}

struct PriorityStoreInner {
    items: Vec<Py<PyAny>>,
    put_queue: VecDeque<Py<StorePut>>,
    get_queue: VecDeque<Py<StoreGet>>,
}

/// Store where items are retrieved in priority order (smallest item first).
#[pyclass(subclass, module = "simpy_rs._simpy_rs")]
pub struct PriorityStore {
    pub env: Py<Environment>,
    pub capacity: f64,
    inner: Mutex<PriorityStoreInner>,
}

impl PriorityStore {
    pub fn trigger_put_rs(store_py: &Py<PriorityStore>, py: Python<'_>) -> PyResult<()> {
        loop {
            let next_put = {
                let s = store_py.borrow(py);
                let mut inner = s.inner.lock().unwrap();
                if (inner.items.len() as f64) < s.capacity {
                    inner.put_queue.pop_front()
                } else {
                    None
                }
            };
            match next_put {
                Some(put_ev) => {
                    let item = put_ev.borrow(py).item.clone_ref(py);
                    // Insert in sorted position outside lock? We need comparison in Python, so snapshot or find index.
                    // Snapshot items under lock to compare safely without holding lock during Python `<` call.
                    let snapshot: Vec<Py<PyAny>> = {
                        let s = store_py.borrow(py);
                        let inner = s.inner.lock().unwrap();
                        inner.items.iter().map(|i| i.clone_ref(py)).collect()
                    };
                    let mut insert_pos = snapshot.len();
                    for (idx, existing) in snapshot.iter().enumerate() {
                        if item.bind(py).lt(existing.bind(py))? {
                            insert_pos = idx;
                            break;
                        }
                    }
                    {
                        let s = store_py.borrow(py);
                        let mut inner = s.inner.lock().unwrap();
                        let pos = insert_pos.min(inner.items.len());
                        inner.items.insert(pos, item);
                    }
                    let ev_py: Py<Event> = put_ev.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, py.None())?;
                }
                None => break,
            }
        }
        Ok(())
    }

    pub fn trigger_get_rs(store_py: &Py<PriorityStore>, py: Python<'_>) -> PyResult<()> {
        loop {
            let next_get = {
                let s = store_py.borrow(py);
                let mut inner = s.inner.lock().unwrap();
                if !inner.items.is_empty() {
                    if let Some(get_ev) = inner.get_queue.pop_front() {
                        let item = inner.items.remove(0);
                        Some((get_ev, item))
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            match next_get {
                Some((get_ev, item)) => {
                    let ev_py: Py<Event> = get_ev.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, item)?;
                }
                None => break,
            }
        }
        Ok(())
    }
}

#[pymethods]
impl PriorityStore {
    #[new]
    #[pyo3(signature = (env, capacity = f64::INFINITY))]
    fn py_new(env: Py<Environment>, capacity: f64) -> PyResult<Self> {
        if capacity.is_nan() || capacity <= 0.0 {
            return Err(PyValueError::new_err("\"capacity\" must be > 0."));
        }
        Ok(Self {
            env,
            capacity,
            inner: Mutex::new(PriorityStoreInner {
                items: Vec::new(),
                put_queue: VecDeque::new(),
                get_queue: VecDeque::new(),
            }),
        })
    }

    #[getter]
    fn capacity(&self) -> f64 {
        self.capacity
    }

    #[getter]
    fn items(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, PyAny>> = inner.items.iter().map(|i| i.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn put_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, StorePut>> = inner.put_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn get_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, StoreGet>> = inner.get_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    fn put(slf: Bound<'_, Self>, py: Python<'_>, item: Py<PyAny>) -> PyResult<Py<StorePut>> {
        let env = slf.borrow().env.clone_ref(py);
        let proc = env.borrow(py).active_process_rs(py);
        let store_py = slf.clone().unbind();
        let put_ev = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(StorePut {
                resource: store_py.clone_ref(py).into_any(),
                item,
                proc,
            }),
        )?;
        put_ev
            .bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::PriorityStoreGet(store_py.clone_ref(py)));
        {
            let s = slf.borrow();
            s.inner.lock().unwrap().put_queue.push_back(put_ev.clone_ref(py));
        }
        Self::trigger_put_rs(&store_py, py)?;
        Ok(put_ev)
    }

    fn get(slf: Bound<'_, Self>, py: Python<'_>) -> PyResult<Py<StoreGet>> {
        let env = slf.borrow().env.clone_ref(py);
        let proc = env.borrow(py).active_process_rs(py);
        let store_py = slf.clone().unbind();
        let get_ev = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(StoreGet {
                resource: store_py.clone_ref(py).into_any(),
                proc,
            }),
        )?;
        get_ev
            .bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::PriorityStorePut(store_py.clone_ref(py)));
        {
            let s = slf.borrow();
            s.inner.lock().unwrap().get_queue.push_back(get_ev.clone_ref(py));
        }
        Self::trigger_get_rs(&store_py, py)?;
        Ok(get_ev)
    }
}

#[pyclass(extends = Event, subclass, module = "simpy_rs._simpy_rs")]
pub struct FilterStoreGet {
    pub resource: Py<FilterStore>,
    pub filter: Option<Py<PyAny>>,
    pub proc: Option<Py<Process>>,
}

#[pymethods]
impl FilterStoreGet {
    #[getter]
    fn filter(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.filter.as_ref().map(|f| f.clone_ref(py))
    }

    #[getter]
    fn proc(&self, py: Python<'_>) -> Option<Py<Process>> {
        self.proc.as_ref().map(|p| p.clone_ref(py))
    }

    fn cancel(slf: Bound<'_, Self>, py: Python<'_>) {
        if !slf.as_super().borrow().is_triggered() {
            let store = slf.borrow().resource.clone_ref(py);
            let me = slf.unbind();
            store.borrow(py).inner.lock().unwrap().get_queue.retain(|g| !g.is(&me));
        }
    }

    fn __enter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    #[pyo3(signature = (_exc_type = None, _exc_val = None, _exc_tb = None))]
    fn __exit__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        _exc_type: Option<Bound<'_, PyAny>>,
        _exc_val: Option<Bound<'_, PyAny>>,
        _exc_tb: Option<Bound<'_, PyAny>>,
    ) -> bool {
        Self::cancel(slf, py);
        false
    }
}

struct FilterStoreInner {
    items: Vec<Py<PyAny>>,
    put_queue: VecDeque<Py<StorePut>>,
    get_queue: Vec<Py<FilterStoreGet>>,
}

/// Store where `get(filter=...)` retrieves the first item matching a predicate function.
#[pyclass(subclass, module = "simpy_rs._simpy_rs")]
pub struct FilterStore {
    pub env: Py<Environment>,
    pub capacity: f64,
    inner: Mutex<FilterStoreInner>,
}

impl FilterStore {
    pub fn trigger_put_rs(store_py: &Py<FilterStore>, py: Python<'_>) -> PyResult<()> {
        loop {
            let next_put = {
                let s = store_py.borrow(py);
                let mut inner = s.inner.lock().unwrap();
                if (inner.items.len() as f64) < s.capacity {
                    if let Some(put_ev) = inner.put_queue.pop_front() {
                        let item = put_ev.borrow(py).item.clone_ref(py);
                        inner.items.push(item);
                        Some(put_ev)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            match next_put {
                Some(put_ev) => {
                    let ev_py: Py<Event> = put_ev.bind(py).as_super().clone().unbind();
                    Event::succeed_rs(&ev_py, py, py.None())?;
                }
                None => break,
            }
        }
        Ok(())
    }

    pub fn trigger_get_rs(store_py: &Py<FilterStore>, py: Python<'_>) -> PyResult<()> {
        let mut idx = 0;
        loop {
            let candidate = {
                let s = store_py.borrow(py);
                let inner = s.inner.lock().unwrap();
                if idx < inner.get_queue.len() && !inner.items.is_empty() {
                    let get_ev = inner.get_queue[idx].clone_ref(py);
                    let items_snapshot: Vec<Py<PyAny>> =
                        inner.items.iter().map(|i| i.clone_ref(py)).collect();
                    Some((get_ev, items_snapshot))
                } else {
                    None
                }
            };
            let Some((get_ev, items_snapshot)) = candidate else {
                break;
            };

            let filter_opt = get_ev.borrow(py).filter.as_ref().map(|f| f.clone_ref(py));
            let mut matched_idx = None;
            for (item_i, item) in items_snapshot.iter().enumerate() {
                let matches = match &filter_opt {
                    None => true,
                    Some(f) => f.bind(py).call1((item.bind(py),))?.is_truthy()?,
                };
                if matches {
                    matched_idx = Some(item_i);
                    break;
                }
            }

            if let Some(item_i) = matched_idx {
                let removed_item = {
                    let s = store_py.borrow(py);
                    let mut inner = s.inner.lock().unwrap();
                    if idx < inner.get_queue.len() && inner.get_queue[idx].is(&get_ev) {
                        inner.get_queue.remove(idx);
                    }
                    inner.items.remove(item_i)
                };
                let ev_py: Py<Event> = get_ev.bind(py).as_super().clone().unbind();
                Event::succeed_rs(&ev_py, py, removed_item)?;
            } else {
                idx += 1;
            }
        }
        Ok(())
    }
}

#[pymethods]
impl FilterStore {
    #[new]
    #[pyo3(signature = (env, capacity = f64::INFINITY))]
    fn py_new(env: Py<Environment>, capacity: f64) -> PyResult<Self> {
        if capacity.is_nan() || capacity <= 0.0 {
            return Err(PyValueError::new_err("\"capacity\" must be > 0."));
        }
        Ok(Self {
            env,
            capacity,
            inner: Mutex::new(FilterStoreInner {
                items: Vec::new(),
                put_queue: VecDeque::new(),
                get_queue: Vec::new(),
            }),
        })
    }

    #[getter]
    fn capacity(&self) -> f64 {
        self.capacity
    }

    #[getter]
    fn items(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, PyAny>> = inner.items.iter().map(|i| i.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn put_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, StorePut>> = inner.put_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    #[getter]
    fn get_queue(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        let items: Vec<&Bound<'_, FilterStoreGet>> =
            inner.get_queue.iter().map(|q| q.bind(py)).collect();
        Ok(PyList::new(py, items)?.unbind())
    }

    fn put(slf: Bound<'_, Self>, py: Python<'_>, item: Py<PyAny>) -> PyResult<Py<StorePut>> {
        let env = slf.borrow().env.clone_ref(py);
        let proc = env.borrow(py).active_process_rs(py);
        let store_py = slf.clone().unbind();
        let put_ev = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(StorePut {
                resource: store_py.clone_ref(py).into_any(),
                item,
                proc,
            }),
        )?;
        put_ev
            .bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::FilterStoreGet(store_py.clone_ref(py)));
        {
            let s = slf.borrow();
            s.inner.lock().unwrap().put_queue.push_back(put_ev.clone_ref(py));
        }
        Self::trigger_put_rs(&store_py, py)?;
        Ok(put_ev)
    }

    #[pyo3(signature = (filter = None))]
    fn get(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        filter: Option<Py<PyAny>>,
    ) -> PyResult<Py<FilterStoreGet>> {
        let env = slf.borrow().env.clone_ref(py);
        let proc = env.borrow(py).active_process_rs(py);
        let store_py = slf.clone().unbind();
        let get_ev = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(FilterStoreGet {
                resource: store_py.clone_ref(py),
                filter,
                proc,
            }),
        )?;
        get_ev
            .bind(py)
            .as_super()
            .borrow()
            .add_internal_callback(InternalCallback::FilterStorePut(store_py.clone_ref(py)));
        {
            let s = slf.borrow();
            s.inner.lock().unwrap().get_queue.push(get_ev.clone_ref(py));
        }
        Self::trigger_get_rs(&store_py, py)?;
        Ok(get_ev)
    }
}
