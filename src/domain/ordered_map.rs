//! An insertion-ordered map with JavaScript `Map` semantics: setting an
//! existing key keeps its position, deleting frees it.

use std::collections::HashMap;
use std::hash::Hash;

#[derive(Clone, Debug)]
pub struct OrderedMap<K, V> {
    slots: Vec<Option<(K, V)>>,
    index: HashMap<K, usize>,
}

impl<K: Clone + Eq + Hash, V> Default for OrderedMap<K, V> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            index: HashMap::new(),
        }
    }
}

impl<K: Clone + Eq + Hash, V> OrderedMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.index.contains_key(key)
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.index
            .get(key)
            .and_then(|&i| self.slots[i].as_ref())
            .map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let i = *self.index.get(key)?;
        self.slots[i].as_mut().map(|(_, v)| v)
    }

    /// `map.set(key, value)`.
    pub fn insert(&mut self, key: K, value: V) {
        if let Some(&i) = self.index.get(&key) {
            if let Some(slot) = self.slots[i].as_mut() {
                slot.1 = value;
            }
            return;
        }
        self.index.insert(key.clone(), self.slots.len());
        self.slots.push(Some((key, value)));
    }

    /// The value for `key`, inserting `default()` at the end when absent.
    pub fn entry_or_insert_with(&mut self, key: K, default: impl FnOnce() -> V) -> &mut V {
        if !self.index.contains_key(&key) {
            self.insert(key.clone(), default());
        }
        self.get_mut(&key).expect("just inserted")
    }

    /// `map.delete(key)`.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let i = self.index.remove(key)?;
        self.slots[i].take().map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.slots.iter().flatten().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }

    pub fn into_entries(self) -> impl Iterator<Item = (K, V)> {
        self.slots.into_iter().flatten()
    }
}

impl<K: Clone + Eq + Hash, V> FromIterator<(K, V)> for OrderedMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = Self::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}
