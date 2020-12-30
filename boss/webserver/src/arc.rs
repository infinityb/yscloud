// Copyright 2020 Stacey Ell
//
// This software is dual-licensed under MIT/Apache2.0
//
// MIT:
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// 
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// 
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// Apache 2.0:
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};

use linked_hash_map::LinkedHashMap;

#[derive(Debug)]
pub struct Cache<K, V, S = RandomState>
where
    K: Hash + Eq + Clone,
    S: BuildHasher,
{
    capacity: usize,
    p_value: usize,

    // aka T_1 from the paper.
    recent_top: LinkedHashMap<K, V, S>,
    // aka B_1 from the paper.
    recent_bottom: LinkedHashMap<K, (), S>,
    // aka T_2 from the paper.
    frequent_top: LinkedHashMap<K, V, S>,
    // aka B_2 from the paper.
    frequent_bottom: LinkedHashMap<K, (), S>,
}

impl<K, V> Cache<K, V>
where
    K: Hash + Eq + Clone,
{
    pub fn new(capacity: usize) -> Cache<K, V> {
        Cache {
            capacity,
            p_value: 0,
            recent_top: Default::default(),
            recent_bottom: Default::default(),
            frequent_top: Default::default(),
            frequent_bottom: Default::default(),
        }
    }
}

impl<K, V, S> Cache<K, V, S>
where
    K: Hash + Eq + Clone,
    S: BuildHasher,
{
    fn replace_helper(&mut self, was_in_bottom: bool, evicted: Option<&mut Vec<(K, V)>>) {
        let mut operate_on_recent = false;

        if self.recent_top.len() > self.p_value {
            operate_on_recent = true;
        } else if self.recent_top.len() == self.p_value && !self.recent_top.is_empty() {
            operate_on_recent = was_in_bottom;
        }

        if operate_on_recent {
            let (k, _v) = self.recent_top.pop_front().unwrap();

            while self.capacity <= self.recent_bottom.len() {
                self.recent_bottom.pop_front();
            }

            self.recent_bottom.insert(k, ());
        } else {
            let (k, v) = self.frequent_top.pop_front().unwrap();

            if let Some(f) = evicted {
                f.push((k.clone(), v));
            }

            while self.capacity <= self.frequent_top.len() {
                self.frequent_bottom.pop_front();
            }

            self.frequent_bottom.insert(k, ());
        }
    }

    pub fn get_norefresh(&self, key: &'_ K) -> Option<&V> {
        if let Some(v) = self.frequent_top.get(key) {
            return Some(v);
        }
        self.recent_top.get(key)
    }

    pub fn get_refresh(&mut self, key: &'_ K) -> Option<&V> {
        if let Some(v) = self.recent_top.remove(key) {
            if let linked_hash_map::Entry::Vacant(vac) = self.frequent_top.entry(key.clone()) {
                return Some(vac.insert(v));
            } else {
                unreachable!("invariant broken");
            }
        }

        if let Some(v) = self.frequent_top.get_refresh(key) {
            return Some(v);
        }

        None
    }

    pub fn insert(&mut self, key: K, value: V, evicted: Option<&mut Vec<(K, V)>>) -> Option<V> {
        let mut inject_frequent = false;

        let recent_top_vacant = match self.recent_top.entry(key) {
            linked_hash_map::Entry::Occupied(mut occ) => return Some(occ.insert(value)),
            linked_hash_map::Entry::Vacant(vac) => vac,
        };
        let frequent_top_vacant = match self.frequent_top.entry(recent_top_vacant.key().clone()) {
            linked_hash_map::Entry::Occupied(mut occ) => return Some(occ.insert(value)),
            linked_hash_map::Entry::Vacant(vac) => vac,
        };

        let mut was_in_frequent_bottom = false;
        if self
            .frequent_bottom
            .remove(recent_top_vacant.key())
            .is_some()
        {
            let d = if self.recent_bottom.len() <= self.frequent_bottom.len() {
                1
            } else {
                self.recent_bottom.len() / self.frequent_bottom.len()
            };

            if d <= self.p_value {
                self.p_value -= d;
            } else {
                self.p_value = 0;
            }

            was_in_frequent_bottom = true;

            inject_frequent = true;
        } else if self.recent_bottom.remove(recent_top_vacant.key()).is_some() {
            let d = if self.frequent_bottom.len() <= self.recent_bottom.len() {
                1
            } else {
                self.frequent_bottom.len() / self.recent_bottom.len()
            };

            self.p_value += d;
            if self.capacity < self.p_value {
                self.p_value = self.capacity;
            }

            inject_frequent = true;
        }

        if inject_frequent {
            frequent_top_vacant.insert(value);
        } else {
            recent_top_vacant.insert(value);
        }

        if self.recent_top.len() + self.frequent_top.len() > self.capacity || inject_frequent {
            self.replace_helper(was_in_frequent_bottom, evicted);
        }

        None
    }
}
