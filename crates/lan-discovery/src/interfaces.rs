//! 网卡枚举：本机 IPv4 与定向广播目标。
//!
//! TS 用 `os.networkInterfaces()`；Rust 侧按平台直接读系统接口：
//! - Windows：`GetAdaptersAddresses(AF_INET, GAA_FLAG_INCLUDE_PREFIX)`，前缀长度换算掩码；
//! - Unix：`getifaddrs` + `sockaddr_in`/`ifa_netmask`，按 `IFF_UP` 过滤。
//!
//! 只暴露发现所需的保守集合：本机地址包含回环；广播目标交给
//! [`crate::collect_broadcast_targets`] 去重并补全局广播。

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInterface {
    pub address: String,
    pub netmask: Option<String>,
    pub internal: bool,
}

/// 本机 IPv4 地址集（含 `127.0.0.1`）。
pub fn local_ipv4_addresses() -> HashSet<String> {
    let mut addresses = HashSet::from(["127.0.0.1".to_string()]);
    for interface in network_interfaces() {
        addresses.insert(interface.address);
    }
    addresses
}

/// 定向广播目标推导（不含全局 `255.255.255.255`；由 `collect_broadcast_targets` 补）。
pub fn directed_broadcast_targets() -> Vec<String> {
    crate::collect_broadcast_targets(
        network_interfaces()
            .into_iter()
            .map(|interface| (interface.address, interface.netmask, interface.internal)),
    )
}

#[cfg(windows)]
pub fn network_interfaces() -> Vec<NetworkInterface> {
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_INCLUDE_PREFIX, IP_ADAPTER_ADDRESSES_LH,
    };

    const AF_INET: u32 = 2;
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;

    let mut size: u32 = 15 * 1024;
    let mut buffer: Vec<usize>;
    loop {
        let words = (size as usize).div_ceil(size_of::<usize>());
        buffer = vec![0; words];
        let result = unsafe {
            GetAdaptersAddresses(
                AF_INET,
                GAA_FLAG_INCLUDE_PREFIX,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                &mut size,
            )
        };
        if result == 0 {
            break;
        }
        if result != ERROR_BUFFER_OVERFLOW {
            return Vec::new();
        }
    }

    let mut interfaces = Vec::new();
    let mut adapter = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while !adapter.is_null() {
        let current = unsafe { &*adapter };
        let is_up = current.OperStatus == 1; // IfOperStatusUp
        let is_loopback = current.IfType == IF_TYPE_SOFTWARE_LOOPBACK;
        if is_up && !is_loopback {
            let mut unicast = current.FirstUnicastAddress;
            while !unicast.is_null() {
                let address = unsafe { &*unicast };
                if let Some(ip) =
                    unsafe { read_sockaddr_in(address.Address.lpSockaddr as *const c_void) }
                {
                    interfaces.push(NetworkInterface {
                        address: ip,
                        netmask: prefix_to_netmask(address.OnLinkPrefixLength as u32),
                        internal: false,
                    });
                }
                unicast = address.Next;
            }
        }
        adapter = current.Next;
    }
    interfaces
}

/// `SOCKADDR_IN` 的字段访问用原始偏移，避免依赖 windows-sys 版本的联合体命名：
/// family(u16) + port(u16) + addr(u32, 网络序) + zero[8]。
#[cfg(windows)]
unsafe fn read_sockaddr_in(pointer: *const std::ffi::c_void) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let bytes = pointer.cast::<u8>();
    let family = u16::from_ne_bytes([*bytes, *bytes.add(1)]);
    if family != 2 {
        return None;
    }
    let address = u32::from_ne_bytes([
        *bytes.add(4),
        *bytes.add(5),
        *bytes.add(6),
        *bytes.add(7),
    ]);
    Some(std::net::Ipv4Addr::from(address).to_string())
}

#[cfg(windows)]
fn prefix_to_netmask(prefix: u32) -> Option<String> {
    if prefix > 32 {
        return None;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    Some(std::net::Ipv4Addr::from(mask).to_string())
}

#[cfg(unix)]
pub fn network_interfaces() -> Vec<NetworkInterface> {
    use std::net::Ipv4Addr;

    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Vec::new();
    }
    let mut interfaces = Vec::new();
    let mut current = head;
    while !current.is_null() {
        let entry = unsafe { &*current };
        let internal = entry.ifa_flags & libc::IFF_LOOPBACK as u32 != 0;
        let up = entry.ifa_flags & libc::IFF_UP as u32 != 0;
        if up {
            if let Some(address) = unsafe { ipv4_from_sockaddr(entry.ifa_addr) } {
                let netmask = unsafe { ipv4_from_sockaddr(entry.ifa_netmask) };
                interfaces.push(NetworkInterface {
                    address,
                    netmask,
                    internal,
                });
            }
        }
        current = entry.ifa_next;
    }
    unsafe { libc::freeifaddrs(head) };
    // 保证回环地址在集合里（部分系统不把 lo 暴露为 AF_INET）。
    if !interfaces.iter().any(|interface| interface.address == "127.0.0.1") {
        interfaces.push(NetworkInterface {
            address: Ipv4Addr::LOCALHOST.to_string(),
            netmask: Some("255.0.0.0".to_string()),
            internal: true,
        });
    }
    interfaces
}

#[cfg(unix)]
unsafe fn ipv4_from_sockaddr(pointer: *const libc::sockaddr) -> Option<String> {
    if pointer.is_null() || (*pointer).sa_family as i32 != libc::AF_INET {
        return None;
    }
    let address = &*(pointer as *const libc::sockaddr_in);
    Some(std::net::Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr)).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_addresses_always_include_loopback() {
        assert!(local_ipv4_addresses().contains("127.0.0.1"));
    }

    #[test]
    fn directed_broadcasts_are_valid_ipv4() {
        for target in directed_broadcast_targets() {
            assert!(
                crate::is_ipv4_address(&target),
                "unexpected broadcast target {target}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn prefix_to_netmask_matches_ipv4_masks() {
        assert_eq!(prefix_to_netmask(24).as_deref(), Some("255.255.255.0"));
        assert_eq!(prefix_to_netmask(16).as_deref(), Some("255.255.0.0"));
        assert_eq!(prefix_to_netmask(0).as_deref(), Some("0.0.0.0"));
        assert_eq!(prefix_to_netmask(33), None);
    }
}
