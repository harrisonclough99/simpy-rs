mod env;
mod event;
mod exceptions;
mod extensions;
mod process;
mod resources;

use pyo3::prelude::*;

use env::Environment;
use event::{AllOf, AnyOf, Condition, ConditionValue, Event, Initialize, Interruption, Pending, Timeout, NORMAL, URGENT};
use extensions::{simulate_mm1, simulate_mmc, Monitor};
use process::Process;
use resources::{
    Container, ContainerGet, ContainerPut, FilterStore, FilterStoreGet, Preempted,
    PreemptiveResource, PriorityItem, PriorityRequest, PriorityResource, PriorityStore, Release,
    Request, Resource, Store, StoreGet, StorePut,
};

#[pymodule]
fn _simpy_rs(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("URGENT", URGENT)?;
    m.add("NORMAL", NORMAL)?;
    m.add("Infinity", f64::INFINITY)?;
    m.add("PENDING", Py::new(py, Pending)?)?;

    exceptions::register(py, m)?;

    m.add_class::<Environment>()?;
    m.add_class::<Event>()?;
    m.add_class::<Timeout>()?;
    m.add_class::<Initialize>()?;
    m.add_class::<Interruption>()?;
    m.add_class::<Process>()?;
    m.add_class::<Condition>()?;
    m.add_class::<AllOf>()?;
    m.add_class::<AnyOf>()?;
    m.add_class::<ConditionValue>()?;

    m.add_class::<Resource>()?;
    m.add_class::<Request>()?;
    m.add_class::<Release>()?;
    m.add_class::<PriorityResource>()?;
    m.add_class::<PriorityRequest>()?;
    m.add_class::<PreemptiveResource>()?;
    m.add_class::<Preempted>()?;

    m.add_class::<Container>()?;
    m.add_class::<ContainerPut>()?;
    m.add_class::<ContainerGet>()?;

    m.add_class::<Store>()?;
    m.add_class::<StorePut>()?;
    m.add_class::<StoreGet>()?;
    m.add_class::<PriorityStore>()?;
    m.add_class::<PriorityItem>()?;
    m.add_class::<FilterStore>()?;
    m.add_class::<FilterStoreGet>()?;

    m.add_class::<Monitor>()?;
    m.add_function(wrap_pyfunction!(simulate_mm1, m)?)?;
    m.add_function(wrap_pyfunction!(simulate_mmc, m)?)?;

    Ok(())
}
