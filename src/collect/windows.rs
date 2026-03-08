#[derive(Debug, Clone, Default)]
pub(crate) struct DiskDeviceMetric {
    pub key: String,
    pub label: String,
    pub read_bytes_per_sec: Option<f64>,
    pub write_bytes_per_sec: Option<f64>,
    pub busy_percent: Option<f32>,
    pub avg_latency_ms: Option<f32>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DiskTotals {
    pub read_bytes_per_sec: Option<f64>,
    pub write_bytes_per_sec: Option<f64>,
    pub busy_percent: Option<f32>,
    pub avg_latency_ms: Option<f32>,
}

#[cfg(windows)]
mod imp {
    use std::mem;

    use windows::core::{w, PCWSTR, PWSTR};
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhEnumObjectItemsW,
        PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA,
        PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_MORE_DATA, PERF_DETAIL_WIZARD,
    };

    use super::{DiskDeviceMetric, DiskTotals};

    pub(super) struct DiskQuery {
        query: isize,
        total_busy_counter: isize,
        total_latency_counter: isize,
        total_read_counter: isize,
        total_write_counter: isize,
        instances: Vec<DiskInstanceCounters>,
        initialized: bool,
    }

    struct DiskInstanceCounters {
        key: String,
        label: String,
        busy_counter: isize,
        latency_counter: isize,
        read_counter: isize,
        write_counter: isize,
    }

    impl DiskQuery {
        pub(super) fn new() -> Option<Self> {
            unsafe {
                let mut query = 0isize;
                if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                    return None;
                }

                let total_busy_counter = add_counter(query, "\\PhysicalDisk(_Total)\\% Disk Time")?;
                let total_latency_counter =
                    add_counter(query, "\\PhysicalDisk(_Total)\\Avg. Disk sec/Transfer")?;
                let total_read_counter =
                    add_counter(query, "\\PhysicalDisk(_Total)\\Disk Read Bytes/sec")?;
                let total_write_counter =
                    add_counter(query, "\\PhysicalDisk(_Total)\\Disk Write Bytes/sec")?;

                let instances = enumerate_physical_disk_instances()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|name| name != "_Total")
                    .filter_map(|name| build_instance_counters(query, &name))
                    .collect::<Vec<_>>();

                let _ = PdhCollectQueryData(query);

                Some(Self {
                    query,
                    total_busy_counter,
                    total_latency_counter,
                    total_read_counter,
                    total_write_counter,
                    instances,
                    initialized: false,
                })
            }
        }

        pub(super) fn sample(&mut self) -> (DiskTotals, Vec<DiskDeviceMetric>) {
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return (DiskTotals::default(), Vec::new());
                }

                if !self.initialized {
                    self.initialized = true;
                    return (DiskTotals::default(), Vec::new());
                }

                let totals = DiskTotals {
                    read_bytes_per_sec: read_counter_double(self.total_read_counter),
                    write_bytes_per_sec: read_counter_double(self.total_write_counter),
                    busy_percent: read_counter_double(self.total_busy_counter)
                        .map(|v| v.clamp(0.0, 100.0) as f32),
                    avg_latency_ms: read_counter_double(self.total_latency_counter)
                        .map(|v| (v.max(0.0) * 1000.0) as f32),
                };

                let per_disk = self
                    .instances
                    .iter()
                    .map(|c| DiskDeviceMetric {
                        key: c.key.clone(),
                        label: c.label.clone(),
                        read_bytes_per_sec: read_counter_double(c.read_counter),
                        write_bytes_per_sec: read_counter_double(c.write_counter),
                        busy_percent: read_counter_double(c.busy_counter)
                            .map(|v| v.clamp(0.0, 100.0) as f32),
                        avg_latency_ms: read_counter_double(c.latency_counter)
                            .map(|v| (v.max(0.0) * 1000.0) as f32),
                    })
                    .collect();

                (totals, per_disk)
            }
        }
    }

    impl Drop for DiskQuery {
        fn drop(&mut self) {
            unsafe {
                let _ = PdhCloseQuery(self.query);
            }
        }
    }

    unsafe fn add_counter(query: isize, path: &str) -> Option<isize> {
        let mut counter = 0isize;
        let wide = to_wide(path);
        if PdhAddEnglishCounterW(query, PCWSTR(wide.as_ptr()), 0, &mut counter) == 0 {
            Some(counter)
        } else {
            None
        }
    }

    unsafe fn build_instance_counters(
        query: isize,
        instance: &str,
    ) -> Option<DiskInstanceCounters> {
        let busy_counter = add_counter(query, &format!("\\PhysicalDisk({instance})\\% Disk Time"))?;
        let latency_counter = add_counter(
            query,
            &format!("\\PhysicalDisk({instance})\\Avg. Disk sec/Transfer"),
        )?;
        let read_counter = add_counter(
            query,
            &format!("\\PhysicalDisk({instance})\\Disk Read Bytes/sec"),
        )?;
        let write_counter = add_counter(
            query,
            &format!("\\PhysicalDisk({instance})\\Disk Write Bytes/sec"),
        )?;

        Some(DiskInstanceCounters {
            key: normalize_instance_key(instance),
            label: instance.to_string(),
            busy_counter,
            latency_counter,
            read_counter,
            write_counter,
        })
    }

    unsafe fn enumerate_physical_disk_instances() -> Option<Vec<String>> {
        let mut counter_buf_len = 0u32;
        let mut instance_buf_len = 0u32;

        let status = PdhEnumObjectItemsW(
            PCWSTR::null(),
            PCWSTR::null(),
            w!("PhysicalDisk"),
            PWSTR::null(),
            &mut counter_buf_len,
            PWSTR::null(),
            &mut instance_buf_len,
            PERF_DETAIL_WIZARD,
            0,
        );

        if status != PDH_MORE_DATA {
            return None;
        }

        let mut counter_buf = vec![0u16; counter_buf_len as usize];
        let mut instance_buf = vec![0u16; instance_buf_len as usize];

        let status = PdhEnumObjectItemsW(
            PCWSTR::null(),
            PCWSTR::null(),
            w!("PhysicalDisk"),
            PWSTR(counter_buf.as_mut_ptr()),
            &mut counter_buf_len,
            PWSTR(instance_buf.as_mut_ptr()),
            &mut instance_buf_len,
            PERF_DETAIL_WIZARD,
            0,
        );

        if status != 0 {
            return None;
        }

        Some(parse_multi_sz(&instance_buf))
    }

    fn parse_multi_sz(buf: &[u16]) -> Vec<String> {
        let mut out = Vec::new();
        let mut start = 0usize;
        for i in 0..buf.len() {
            if buf[i] == 0 {
                if i == start {
                    break;
                }
                if let Ok(s) = String::from_utf16(&buf[start..i]) {
                    out.push(s);
                }
                start = i + 1;
            }
        }
        out
    }

    fn normalize_instance_key(instance: &str) -> String {
        for token in instance.split_whitespace().rev() {
            let t = token.trim();
            if t.ends_with(':') && t.len() >= 2 {
                return t.to_ascii_uppercase();
            }
        }

        if let Some(idx) = instance.find(':') {
            if idx >= 1 {
                let letter = instance.as_bytes()[idx - 1] as char;
                if letter.is_ascii_alphabetic() {
                    return format!("{}:", letter.to_ascii_uppercase());
                }
            }
        }

        instance.to_string()
    }

    unsafe fn read_counter_double(counter: isize) -> Option<f64> {
        let mut counter_type = 0u32;
        let mut value: PDH_FMT_COUNTERVALUE = mem::zeroed();

        if PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, Some(&mut counter_type), &mut value)
            != 0
        {
            return None;
        }

        if value.CStatus != PDH_CSTATUS_VALID_DATA && value.CStatus != PDH_CSTATUS_NEW_DATA {
            return None;
        }

        Some(value.Anonymous.doubleValue)
    }

    fn to_wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

pub(crate) struct WindowsCollector {
    #[cfg(windows)]
    disk_query: Option<imp::DiskQuery>,
}

impl WindowsCollector {
    pub(crate) fn new() -> Self {
        #[cfg(windows)]
        {
            Self {
                disk_query: imp::DiskQuery::new(),
            }
        }

        #[cfg(not(windows))]
        {
            Self {}
        }
    }

    pub(crate) fn query_disk_metrics(&mut self) -> (DiskTotals, Vec<DiskDeviceMetric>) {
        #[cfg(windows)]
        {
            if let Some(query) = self.disk_query.as_mut() {
                return query.sample();
            }
            (DiskTotals::default(), Vec::new())
        }

        #[cfg(not(windows))]
        {
            (DiskTotals::default(), Vec::new())
        }
    }
}
