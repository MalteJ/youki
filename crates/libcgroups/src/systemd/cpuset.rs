use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use dbus::arg::RefArg;
use fixedbitset::FixedBitSet;
use oci_spec::runtime::LinuxCpu;

use crate::common::ControllerOpt;

use super::controller::Controller;

pub const ALLOWED_CPUS: &str = "AllowedCPUs";
pub const ALLOWED_NODES: &str = "AllowedMemoryNodes";

pub struct CpuSet {}

impl Controller for CpuSet {
    fn apply(
        options: &ControllerOpt,
        systemd_version: u32,
        properties: &mut HashMap<&str, Box<dyn RefArg>>,
    ) -> Result<()> {
        if let Some(cpu) = options.resources.cpu() {
            log::debug!("Applying cpuset resource restrictions");
            return Self::apply(cpu, systemd_version, properties)
                .context("could not apply cpuset resource restrictions");
        }

        Ok(())
    }
}

impl CpuSet {
    fn apply(
        cpu: &LinuxCpu,
        systemd_version: u32,
        properties: &mut HashMap<&str, Box<dyn RefArg>>,
    ) -> Result<()> {
        if systemd_version <= 243 {
            bail!("setting cpuset restrictions requires systemd version greather than 243");
        }

        if let Some(cpus) = cpu.cpus() {
            let cpu_mask = to_bitmask(cpus).context("could not create bitmask for cpus")?;
            properties.insert(ALLOWED_CPUS, Box::new(cpu_mask));
        }

        if let Some(mems) = cpu.mems() {
            let mems_mask =
                to_bitmask(mems).context("could not create bitmask for memory nodes")?;
            properties.insert(ALLOWED_NODES, Box::new(mems_mask));
        }

        Ok(())
    }
}

pub fn to_bitmask(range: &str) -> Result<Vec<u8>> {
    let mut bitset = FixedBitSet::with_capacity(8);

    for cpu_set in range.split_terminator(',') {
        let cpu_set = cpu_set.trim();
        if cpu_set.is_empty() {
            continue;
        }

        let cpus: Vec<&str> = cpu_set.split('-').map(|s| s.trim()).collect();
        if cpus.len() == 1 {
            let cpu_index: usize = cpus[0].parse()?;
            if cpu_index >= bitset.len() {
                bitset.grow(bitset.len() + 8);
            }
            bitset.set(cpu_index, true);
        } else {
            let start_index = cpus[0].parse()?;
            let end_index = cpus[1].parse()?;
            if start_index > end_index {
                bail!("invalid cpu range {}", cpu_set);
            }

            if end_index >= bitset.len() {
                bitset.grow(end_index + 1);
            }

            bitset.set_range(start_index..end_index + 1, true);
        }
    }

    // systemd expects a sequence of bytes with no leading zeros, otherwise the values will not be set
    // with no error message
    Ok(bitset
        .as_slice()
        .iter()
        .flat_map(|b| b.to_be_bytes())
        .skip_while(|b| *b == 0u8)
        .collect())
}

#[cfg(test)]
mod tests {
    use dbus::arg::{ArgType, RefArg};
    use oci_spec::runtime::LinuxCpuBuilder;

    use super::*;

    #[test]
    fn to_bitmask_single_value() -> Result<()> {
        let cpus = "0"; // 0000 0001

        let bitmask = to_bitmask(cpus).context("to bitmask")?;

        assert_eq!(bitmask.len(), 1);
        assert_eq!(bitmask[0], 1);
        Ok(())
    }

    #[test]
    fn to_bitmask_multiple_single_values() -> Result<()> {
        let cpus = "0,1,2"; // 0000 0111

        let bitmask = to_bitmask(cpus).context("to bitmask")?;

        assert_eq!(bitmask.len(), 1);
        assert_eq!(bitmask[0], 7);
        Ok(())
    }

    #[test]
    fn to_bitmask_range_value() -> Result<()> {
        let cpus = "0-2"; // 0000 0111

        let bitmask = to_bitmask(cpus).context("to bitmask")?;

        assert_eq!(bitmask.len(), 1);
        assert_eq!(bitmask[0], 7);
        Ok(())
    }

    #[test]
    fn to_bitmask_interchanged_range() -> Result<()> {
        let cpus = "2-0";

        let result = to_bitmask(cpus).context("to bitmask");
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn to_bitmask_incomplete_range() -> Result<()> {
        let cpus = vec!["2-", "-2"];

        for c in cpus {
            let result = to_bitmask(c).context("to bitmask");
            assert!(result.is_err());
        }

        Ok(())
    }

    #[test]
    fn to_bitmask_mixed() -> Result<()> {
        let cpus = "0,2-4,7,9-10"; // 0000 0110 1001 1101

        let bitmask = to_bitmask(cpus).context("to bitmask")?;

        assert_eq!(bitmask.len(), 2);
        assert_eq!(bitmask[0], 6);
        assert_eq!(bitmask[1], 157);
        Ok(())
    }

    #[test]
    fn to_bitmask_extra_characters() -> Result<()> {
        let cpus = "0, 2- 4,,7   ,,9-10"; // 0000 0110 1001 1101

        let bitmask = to_bitmask(cpus).context("to bitmask")?;
        assert_eq!(bitmask.len(), 2);
        assert_eq!(bitmask[0], 6);
        assert_eq!(bitmask[1], 157);

        Ok(())
    }

    #[test]
    fn test_cpuset_systemd_too_old() -> Result<()> {
        let systemd_version = 235;
        let cpu = LinuxCpuBuilder::default()
            .build()
            .context("build cpu spec")?;
        let mut properties: HashMap<&str, Box<dyn RefArg>> = HashMap::new();

        let result = CpuSet::apply(&cpu, systemd_version, &mut properties);

        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_cpuset_set() -> Result<()> {
        let systemd_version = 245;
        let cpu = LinuxCpuBuilder::default()
            .cpus("0-3")
            .mems("0-3")
            .build()
            .context("build cpu spec")?;
        let mut properties: HashMap<&str, Box<dyn RefArg>> = HashMap::new();

        CpuSet::apply(&cpu, systemd_version, &mut properties).context("apply cpuset")?;

        assert_eq!(properties.len(), 2);
        assert!(properties.contains_key(ALLOWED_CPUS));
        let cpus = properties.get(ALLOWED_CPUS).unwrap();
        assert_eq!(cpus.arg_type(), ArgType::Array);

        assert!(properties.contains_key(ALLOWED_NODES));
        let mems = properties.get(ALLOWED_NODES).unwrap();
        assert_eq!(mems.arg_type(), ArgType::Array);

        Ok(())
    }
}
