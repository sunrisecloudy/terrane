use std::any::Any;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::abi::{Error, Result};

/// A typed state store implemented by the host engine's aggregate state.
pub trait StateStore {
    fn get(&self, namespace: &str) -> Option<&dyn Any>;
    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any>;
}

pub fn state_ref<'a, T: 'static>(state: &'a dyn StateStore, namespace: &str) -> Result<&'a T> {
    state
        .get(namespace)
        .and_then(|slice| slice.downcast_ref::<T>())
        .ok_or_else(|| Error::Runtime(format!("missing or invalid {namespace} state slice")))
}

pub fn state_mut<'a, T: 'static>(
    state: &'a mut dyn StateStore,
    namespace: &str,
) -> Result<&'a mut T> {
    state
        .get_mut(namespace)
        .and_then(|slice| slice.downcast_mut::<T>())
        .ok_or_else(|| Error::Runtime(format!("missing or invalid {namespace} state slice")))
}

pub fn snapshot_state<T>(state: &dyn StateStore, namespace: &str) -> Result<Option<Vec<u8>>>
where
    T: BorshSerialize + Default + PartialEq + 'static,
{
    let slice = state_ref::<T>(state, namespace)?;
    if slice == &T::default() {
        return Ok(None);
    }
    borsh::to_vec(slice)
        .map(Some)
        .map_err(|e| Error::Storage(format!("snapshot {namespace}: {e}")))
}

pub fn restore_state<T>(
    state: &mut dyn StateStore,
    namespace: &str,
    payload: &[u8],
) -> Result<()>
where
    T: BorshDeserialize + 'static,
{
    let restored = borsh::from_slice::<T>(payload)
        .map_err(|e| Error::Storage(format!("restore {namespace}: {e}")))?;
    *state_mut::<T>(state, namespace)? = restored;
    Ok(())
}
