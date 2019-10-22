use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::net::{Ipv6Addr, Ipv4Addr};
use std::path::PathBuf;
use std::time::{Instant, Duration};

use rand::rngs::StdRng;
use smallvec::SmallVec;

struct BackendDialerState {
    random_source: StdRng,
    unix_socket_hasher1: RandomState,
    unix_socket_hasher2: RandomState,
    backends: BTreeMap<BackendKeyInternal, BackendStatistics>,
}

#[derive(Debug, Hash, Eq, PartialEq, Clone, Copy)]
struct UnixSocketInfo {
    unix_socket_hash1: u64,
    unix_socket_hash2: u64,
}

#[derive(Debug, Hash, Eq, PartialEq, Clone, Copy)]
enum BackendKeyInternal {
    UnixSocket(UnixSocketInfo),
    TcpV4(Ipv4Addr),
    TcpV6(Ipv6Addr),
}

#[derive(Debug)]
enum BackendKey {
    UnixSocket(PathBuf),
    TcpV4(Ipv4Addr),
    TcpV6(Ipv6Addr),
}

struct BackendStatistics {
    // saturating failure count.
    failure_count: u32,
    attempt_scaling_factor: Duration,
    last_attempt: Instant,
    next_allowed_attempt: Instant,
    /// counts succcessful connections only.  Failed connections are solely
    /// handled by exponential backoff
    handshake_latency: LatencyHistory,
}

struct LatencyHistory {
    data_points: Vec<LatencyDatapoint>,
    ninety_fifth_percentile: Duration,
}

struct LatencyDatapoint {
    when: Instant,
    // None if failed to connect
    latency: Duration,
}

pub struct StartHandle {
    attempt_start: Instant,
}

impl BackendDialerState {
    pub fn create_backend_key_internal(&self, key: &BackendKey) -> BackendKeyInternal {
        use std::hash::{BuildHasher, Hash, Hasher};

        match *key {
            BackendKey::UnixSocket(ref usi) => {
                let mut hasher1 = self.unix_socket_hasher1.build_hasher();
                usi.hash(&mut hasher1);

                let mut hasher2 = self.unix_socket_hasher2.build_hasher();
                usi.hash(&mut hasher2);

                BackendKeyInternal::UnixSocket(UnixSocketInfo {
                    unix_socket_hash1: hasher1.finish(),
                    unix_socket_hash2: hasher2.finish(),
                })
            },
            BackendKey::TcpV4(v) => BackendKeyInternal::TcpV4(v),
            BackendKey::TcpV6(v) => BackendKeyInternal::TcpV6(v),
        }
    }

    fn scrub_expired_backends(&mut self) {
        // remove any backends that haven't been used in half an hour, I guess.
        // via .last_attempt
    }

    pub fn choose_dialers<T>(&mut self, keys: &mut [(T, &BackendKeyInternal)]) {
        use std::cmp::min;

        // what about load shedding?

        const MAX_OUTPUT_BACKENDS: usize = 4;
        let now = Instant::now();

        let mut good_backends = SmallVec::<[usize; MAX_OUTPUT_BACKENDS]>::new();
        let select_count = min(MAX_OUTPUT_BACKENDS, keys.len());

        if good_backends.len() < select_count {
            // find the one with the lowest 95th percentile in recent history
        }
        if good_backends.len() < select_count {
            // find one backend whose statistics are empty or unknown.
        }
        if good_backends.len() < select_count {
            // find a backend randomly, if they have an expired back-off timer
        }
        if good_backends.len() < select_count {
            // find a backend randomly, if they have an expired back-off timer
        }

        for (i, idx) in good_backends.iter().rev().enumerate() {
            keys.swap(i, *idx); // or something like this idk
        }
        // let (idx, _) = keys.iter().enumerate().min_by_key(|(idx, v)| {
        //     if let Some(vv) = self.backends.get(key) {
        //         if vv.next_allowed_attempt < now {
        //             return (0, vv.handshake_latency.ninety_fifth_percentile);
        //         }
        //     }
        //     return (1, vv.handshake_latency.ninety_fifth_percentile)
        // }).unwrap();
        // for (idx, &(_, key)) for keys.iter().enumerate() {
        //     if let Some(vv) = self.backends.get(key) {
        //         if vv.next_allowed_attempt < now {
        //             good_backends.push((vv.handshake_latency.ninety_fifth_percentile, idx));
        //             break;
        //         }
        //     } else {
        //         //
        //     }
        // }

    }

    // pub fn start_dialing(&self, key: &BackendKeyInternal) -> StartHandle {
    //     //
    // }

    // pub fn mark_successful_attempt(&mut self, key: &BackendKeyInternal, sh: StartHandle) {
    //     self.backends.entry(*key) {
    //         //
    //     }
    // }

    // pub fn mark_failed_attempt(&mut self, key: &BackendKeyInternal, sh: StartHandle) {
    //     //
    // }
}

impl BackendStatistics {
    fn mark_successful_attempt(&mut self, start: StartHandle) {
        // If a connection took a while intermitttently, because of load or
        // networking conditions, we might be replacing a newer record with
        // an older one, but I think this is better since these intermittent
        // issues should affect our health.
        self.handshake_latency.update(LatencyDatapoint {
            when: start.attempt_start,
            latency: Instant::now() - start.attempt_start,
        });

    }

    fn mark_failed_event(&mut self, start: StartHandle) {
        self.failure_count += 1;
        if 16 <= self.failure_count {
            self.failure_count = 10;
        }

        let mut delay = self.attempt_scaling_factor;
        for _ in 1..self.failure_count {
            delay += delay;
        }

        let next_allowed_attempt_proposal = start.attempt_start + delay;
        if self.next_allowed_attempt < next_allowed_attempt_proposal {
            self.next_allowed_attempt = next_allowed_attempt_proposal;
        }
    }
}

impl LatencyHistory {
    /// panics if empty.
    fn find_oldest_index(&self) -> usize {
        let mut hist_iter = self.data_points.iter().enumerate();

        let (mut min_idx, mut min_value) = hist_iter.next().unwrap();

        for (i, value) in hist_iter {
            if value.when < min_value.when {
                min_value = value;
                min_idx = i;
            }
        }

        min_idx
    }

    fn update(&mut self, dp: LatencyDatapoint) {
        let max_records = self.data_points.capacity();

        if self.data_points.len() == self.data_points.capacity() {
            // we're full. we need to replace the oldest element.
            let oldest = self.find_oldest_index();
            self.data_points[oldest] = dp;
        } else {
            self.data_points.push(dp);
        }
        self.data_points.sort_by_key(|t| t.latency);
        
        let ninety_fifth_perc_idx = 19 * self.data_points.len() / 20;
        self.ninety_fifth_percentile = self.data_points[ninety_fifth_perc_idx].latency;
    }
}
