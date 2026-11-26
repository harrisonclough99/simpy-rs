use std::sync::Mutex;

use pyo3::exceptions::{PyRuntimeError, PyStopIteration, PyValueError};
use pyo3::intern;
use pyo3::prelude::*;

use crate::env::Environment;
use crate::event::{Event, Initialize, InternalCallback, Interruption};

struct ProcessInner {
    target: Option<Py<Event>>,
}

/// Process of an event-yielding Python generator function.
#[pyclass(extends = Event, subclass, module = "simpy_rs._simpy_rs")]
pub struct Process {
    pub generator: Py<PyAny>,
    send_method: Py<PyAny>,
    throw_method: Py<PyAny>,
    inner: Mutex<ProcessInner>,
}

impl Process {
    pub fn create_rs(
        py: Python<'_>,
        env: Py<Environment>,
        generator: Bound<'_, PyAny>,
    ) -> PyResult<Py<Process>> {
        let throw_method = match generator.getattr(intern!(py, "throw")) {
            Ok(m) => m.unbind(),
            Err(_) => {
                return Err(PyValueError::new_err(format!(
                    "{generator:?} is not a generator."
                )));
            }
        };
        let send_method = generator.getattr(intern!(py, "send"))?.unbind();
        let env_ref = env.clone_ref(py);
        let proc_py = Py::new(
            py,
            PyClassInitializer::from(Event::new_pending(env)).add_subclass(Process {
                generator: generator.unbind(),
                send_method,
                throw_method,
                inner: Mutex::new(ProcessInner { target: None }),
            }),
        )?;
        let init_ev = Initialize::create_and_schedule(py, env_ref, proc_py.clone_ref(py))?;
        let init_as_ev: Py<Event> = init_ev.bind(py).as_super().clone().unbind();
        proc_py.borrow(py).inner.lock().unwrap().target = Some(init_as_ev);
        Ok(proc_py)
    }

    /// Resume this process when `event` is processed. Called directly from Rust's event loop.
    #[inline]
    pub fn resume_rs(proc_py: &Py<Process>, py: Python<'_>, event_py: &Py<Event>) -> PyResult<()> {
        let is_interruption = event_py.borrow(py).inner.lock().unwrap().is_interruption;
        if is_interruption {
            if proc_py.bind(py).as_super().borrow().is_triggered() {
                return Ok(());
            }
            let old_target = {
                let p = proc_py.borrow(py);
                let mut inner = p.inner.lock().unwrap();
                inner.target.take()
            };
            if let Some(target) = old_target {
                target.borrow(py).remove_process_callback(proc_py);
            }
        }

        let env = proc_py.bind(py).as_super().borrow().env.clone_ref(py);
        let (send_method, throw_method) = {
            let p = proc_py.borrow(py);
            (p.send_method.clone_ref(py), p.throw_method.clone_ref(py))
        };

        env.borrow(py).set_active_process(Some(proc_py.clone_ref(py)));

        let mut current_event = event_py.clone_ref(py);

        let result: PyResult<()> = (|| {
            loop {
                let (ev_ok, ev_val) = {
                    let ev = current_event.borrow(py);
                    let mut inner = ev.inner.lock().unwrap();
                    let ok = inner.ok;
                    if !ok {
                        inner.defused = true;
                    }
                    let val = inner
                        .value
                        .as_ref()
                        .map(|v| v.clone_ref(py))
                        .unwrap_or_else(|| py.None());
                    (ok, val)
                };

                let step_res = if ev_ok {
                    send_method.bind(py).call1((ev_val,))
                } else {
                    throw_method.bind(py).call1((ev_val,))
                };

                match step_res {
                    Ok(yielded) => {
                        if let Ok(next_ev_bound) = yielded.cast::<Event>() {
                            let next_ev: Py<Event> = next_ev_bound.clone().unbind();
                            {
                                let p = proc_py.borrow(py);
                                p.inner.lock().unwrap().target = Some(next_ev.clone_ref(py));
                            }
                            let added = next_ev
                                .borrow(py)
                                .add_internal_callback(InternalCallback::Process(proc_py.clone_ref(py)));
                            if !added {
                                current_event = next_ev;
                                continue;
                            }
                            break;
                        } else {
                            let repr = yielded
                                .repr()
                                .map(|r| r.to_string())
                                .unwrap_or_else(|_| "<object>".to_string());
                            let err_exc = PyRuntimeError::new_err(format!(
                                "Invalid yield value \"{repr}\""
                            ))
                            .into_value(py)
                            .into_any();
                            {
                                let p = proc_py.borrow(py);
                                p.inner.lock().unwrap().target = None;
                            }
                            let self_ev: Py<Event> = proc_py.bind(py).as_super().clone().unbind();
                            Event::fail_rs(&self_ev, py, err_exc)?;
                            break;
                        }
                    }
                    Err(err) => {
                        {
                            let p = proc_py.borrow(py);
                            p.inner.lock().unwrap().target = None;
                        }
                        let self_ev: Py<Event> = proc_py.bind(py).as_super().clone().unbind();
                        if err.is_instance_of::<PyStopIteration>(py) {
                            let stop_val = err
                                .value(py)
                                .getattr(intern!(py, "value"))
                                .map(|v| v.unbind())
                                .unwrap_or_else(|_| py.None());
                            Event::succeed_rs(&self_ev, py, stop_val)?;
                        } else {
                            let exc_obj = err.into_value(py).into_any();
                            Event::fail_rs(&self_ev, py, exc_obj)?;
                        }
                        break;
                    }
                }
            }
            Ok(())
        })();

        env.borrow(py).set_active_process(None);
        result
    }
}

#[pymethods]
impl Process {
    #[new]
    fn py_new(
        py: Python<'_>,
        env: Py<Environment>,
        generator: Bound<'_, PyAny>,
    ) -> PyResult<PyClassInitializer<Self>> {
        let throw_method = match generator.getattr(intern!(py, "throw")) {
            Ok(m) => m.unbind(),
            Err(_) => {
                return Err(PyValueError::new_err(format!(
                    "{generator:?} is not a generator."
                )));
            }
        };
        let send_method = generator.getattr(intern!(py, "send"))?.unbind();
        Ok(PyClassInitializer::from(Event::new_pending(env)).add_subclass(Process {
            generator: generator.unbind(),
            send_method,
            throw_method,
            inner: Mutex::new(ProcessInner { target: None }),
        }))
    }

    #[pyo3(signature = (*_args, **_kwargs))]
    fn __init__(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        _args: Bound<'_, pyo3::types::PyTuple>,
        _kwargs: Option<Bound<'_, pyo3::types::PyDict>>,
    ) -> PyResult<()> {
        let env = slf.as_super().borrow().env.clone_ref(py);
        let proc_py = slf.clone().unbind();
        let init_ev = Initialize::create_and_schedule(py, env, proc_py)?;
        let init_as_ev: Py<Event> = init_ev.bind(py).as_super().clone().unbind();
        slf.borrow().inner.lock().unwrap().target = Some(init_as_ev);
        Ok(())
    }

    #[getter]
    fn target(&self, py: Python<'_>) -> Option<Py<Event>> {
        self.inner
            .lock()
            .unwrap()
            .target
            .as_ref()
            .map(|e| e.clone_ref(py))
    }

    #[getter]
    fn name(&self, py: Python<'_>) -> PyResult<String> {
        self.generator
            .bind(py)
            .getattr(intern!(py, "__name__"))?
            .extract()
    }

    #[getter]
    fn is_alive(slf: Bound<'_, Self>) -> bool {
        !slf.as_super().borrow().is_triggered()
    }

    #[pyo3(signature = (cause = None))]
    fn interrupt(
        slf: Bound<'_, Self>,
        py: Python<'_>,
        cause: Option<Py<PyAny>>,
    ) -> PyResult<()> {
        if slf.as_super().borrow().is_triggered() {
            let repr = slf.repr()?;
            return Err(PyRuntimeError::new_err(format!(
                "{repr} has terminated and cannot be interrupted."
            )));
        }
        let env = slf.as_super().borrow().env.clone_ref(py);
        let proc_py = slf.clone().unbind();
        if env.borrow(py).is_active_process(&proc_py) {
            return Err(PyRuntimeError::new_err(
                "A process is not allowed to interrupt itself.",
            ));
        }
        let cause_val = cause.unwrap_or_else(|| py.None());
        Interruption::create_and_schedule(py, env, proc_py, cause_val)?;
        Ok(())
    }

    fn __repr__(slf: Bound<'_, Self>, py: Python<'_>) -> PyResult<String> {
        let name = slf
            .borrow()
            .generator
            .bind(py)
            .getattr(intern!(py, "__name__"))
            .and_then(|n| n.extract::<String>())
            .unwrap_or_else(|_| "generator".to_string());
        let ptr = slf.as_ptr() as usize;
        Ok(format!("<Process({name}) object at {ptr:#x}>"))
    }
}
