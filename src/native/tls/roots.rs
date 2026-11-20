use std::collections::HashMap;
use std::fs;
#[cfg(all(unix, not(target_os = "macos")))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::native::tls::error::TlsError;
use crate::native::tls::pem::decode_pem_certificates;
use crate::native::tls::x509::extract_subject_raw;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootSource {
    System,
    File(PathBuf),
    Inline(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct RootStore {
    ders: Vec<Vec<u8>>,
    by_subject: HashMap<Vec<u8>, Vec<usize>>,
}

static SYSTEM_ROOTS: OnceLock<Result<RootStore, String>> = OnceLock::new();

impl RootStore {
    pub fn from_pem(pem: &[u8]) -> Result<Self, TlsError> {
        let ders = decode_pem_certificates(pem)?;
        if ders.is_empty() {
            return Err(TlsError::NoTrustAnchors);
        }
        Ok(Self::from_ders(ders))
    }

    pub fn from_ders(ders: Vec<Vec<u8>>) -> Self {
        let mut by_subject: HashMap<Vec<u8>, Vec<usize>> = HashMap::new();
        for (idx, der) in ders.iter().enumerate() {
            if let Some(subj) = extract_subject_raw(der) {
                by_subject.entry(subj.to_vec()).or_default().push(idx);
            }
        }
        Self { ders, by_subject }
    }

    pub fn find_by_subject(&self, subject_raw: &[u8]) -> impl Iterator<Item = &[u8]> {
        self.by_subject
            .get(subject_raw)
            .into_iter()
            .flat_map(|indices| indices.iter().map(|&i| self.ders[i].as_slice()))
    }

    pub fn is_empty(&self) -> bool {
        self.ders.is_empty()
    }

    pub fn load(source: &RootSource) -> Result<std::borrow::Cow<'static, RootStore>, TlsError> {
        match source {
            RootSource::File(path) => {
                let bytes = fs::read(path).map_err(|e| {
                    TlsError::CertificateError(format!(
                        "failed to read CA file '{}': {e}",
                        path.display()
                    ))
                })?;
                Ok(std::borrow::Cow::Owned(Self::from_pem(&bytes)?))
            }
            RootSource::Inline(pem) => Ok(std::borrow::Cow::Owned(Self::from_pem(pem)?)),
            RootSource::System => {
                let res = SYSTEM_ROOTS.get_or_init(|| load_system_roots().map_err(|e| e.to_string()));
                match res {
                    Ok(store) => Ok(std::borrow::Cow::Borrowed(store)),
                    Err(_) => Err(TlsError::NoTrustAnchors),
                }
            }
        }
    }
}

fn load_system_roots() -> Result<RootStore, TlsError> {
    // 1. Environment overrides (highest priority)
    for var in ["CONDUIT_CA_FILE", "SSL_CERT_FILE"] {
        if let Ok(path) = std::env::var(var)
            && !path.is_empty()
            && let Ok(bytes) = fs::read(&path)
            && let Ok(store) = RootStore::from_pem(&bytes)
        {
            return Ok(store);
        }
    }

    // 2. Platform-specific trust store
    #[cfg(target_os = "macos")]
    {
        if let Ok(ders) = load_macos_roots_ffi()
            && !ders.is_empty()
        {
            return Ok(RootStore::from_ders(ders));
        }
        if let Ok(ders) = load_macos_roots_cli()
            && !ders.is_empty()
        {
            return Ok(RootStore::from_ders(ders));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(ders) = load_windows_roots()
            && !ders.is_empty()
        {
            return Ok(RootStore::from_ders(ders));
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut all_ders = Vec::new();
        for path in [
            "/etc/ssl/certs/ca-certificates.crt",
            "/etc/pki/tls/certs/ca-bundle.crt",
            "/etc/ssl/cert.pem",
            "/etc/ssl/ca-bundle.pem",
            "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
        ] {
            if let Ok(bytes) = fs::read(path)
                && let Ok(ders) = decode_pem_certificates(&bytes)
                && !ders.is_empty()
            {
                all_ders.extend(ders);
                break;
            }
        }
        if all_ders.is_empty()
            && let Ok(cert_dir) = std::env::var("SSL_CERT_DIR")
        {
            for dir in cert_dir.split(':') {
                if dir.is_empty() {
                    continue;
                }
                load_dir_certs(Path::new(dir), &mut all_ders);
            }
        }
        if !all_ders.is_empty() {
            return Ok(RootStore::from_ders(all_ders));
        }
    }

    Err(TlsError::NoTrustAnchors)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn load_dir_certs(dir: &Path, out: &mut Vec<Vec<u8>>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if matches!(ext, "pem" | "crt")
            && let Ok(bytes) = fs::read(&path)
            && let Ok(ders) = decode_pem_certificates(&bytes)
        {
            out.extend(ders);
        }
    }
}

#[cfg(target_os = "macos")]
fn load_macos_roots_ffi() -> Result<Vec<Vec<u8>>, TlsError> {
    use std::ffi::c_void;

    type CFArrayRef = *const c_void;
    type CFDataRef = *const c_void;
    type CFTypeRef = *const c_void;
    type OSStatus = i32;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecTrustCopyAnchorCertificates(anchors: *mut CFArrayRef) -> OSStatus;
        fn SecCertificateCopyData(certificate: CFTypeRef) -> CFDataRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFArrayGetCount(the_array: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: isize) -> CFTypeRef;
        fn CFDataGetLength(the_data: CFDataRef) -> isize;
        fn CFDataGetBytePtr(the_data: CFDataRef) -> *const u8;
        fn CFRelease(cf: CFTypeRef);
    }

    let mut anchors: CFArrayRef = std::ptr::null();
    // SAFETY: SecTrustCopyAnchorCertificates writes a retained CFArrayRef on status == 0.
    let status = unsafe { SecTrustCopyAnchorCertificates(&mut anchors) };
    if status != 0 || anchors.is_null() {
        return Err(TlsError::NoTrustAnchors);
    }

    let mut ders = Vec::new();
    // SAFETY: anchors is a valid CFArrayRef; every CFDataRef created by SecCertificateCopyData is released.
    unsafe {
        let count = CFArrayGetCount(anchors);
        for i in 0..count {
            let cert = CFArrayGetValueAtIndex(anchors, i);
            if cert.is_null() {
                continue;
            }
            let data = SecCertificateCopyData(cert);
            if data.is_null() {
                continue;
            }
            let len = CFDataGetLength(data);
            let ptr = CFDataGetBytePtr(data);
            if len > 0 && !ptr.is_null() {
                let slice = std::slice::from_raw_parts(ptr, len as usize);
                ders.push(slice.to_vec());
            }
            CFRelease(data);
        }
        CFRelease(anchors);
    }
    Ok(ders)
}

#[cfg(target_os = "macos")]
fn load_macos_roots_cli() -> Result<Vec<Vec<u8>>, TlsError> {
    let out = std::process::Command::new("security")
        .args([
            "find-certificate",
            "-a",
            "-p",
            "/System/Library/Keychains/SystemRootCertificates.keychain",
        ])
        .output()
        .map_err(|_| TlsError::NoTrustAnchors)?;
    if !out.status.success() {
        return Err(TlsError::NoTrustAnchors);
    }
    decode_pem_certificates(&out.stdout)
}

#[cfg(target_os = "windows")]
fn load_windows_roots() -> Result<Vec<Vec<u8>>, TlsError> {
    use std::ffi::c_void;

    #[repr(C)]
    struct CertContext {
        dw_cert_encoding_type: u32,
        pb_cert_encoded: *const u8,
        cb_cert_encoded: u32,
        p_cert_info: *const c_void,
        h_cert_store: *mut c_void,
    }

    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CertOpenSystemStoreW(h_prov: usize, sz_subsystem_protocol: *const u16) -> *mut c_void;
        fn CertEnumCertificatesInStore(
            h_cert_store: *mut c_void,
            p_prev_cert_context: *const CertContext,
        ) -> *const CertContext;
        fn CertCloseStore(h_cert_store: *mut c_void, dw_flags: u32) -> i32;
    }

    let store_name: [u16; 5] = [b'R' as u16, b'O' as u16, b'O' as u16, b'T' as u16, 0];
    // SAFETY: store_name is NUL-terminated UTF-16.
    let store = unsafe { CertOpenSystemStoreW(0, store_name.as_ptr()) };
    if store.is_null() {
        return Err(TlsError::NoTrustAnchors);
    }

    let mut ders = Vec::new();
    // SAFETY: store is valid; CertEnumCertificatesInStore frees previous context automatically when passed back.
    unsafe {
        let mut prev: *const CertContext = std::ptr::null();
        loop {
            let ctx = CertEnumCertificatesInStore(store, prev);
            if ctx.is_null() {
                break;
            }
            let c = &*ctx;
            // X509_ASN_ENCODING == 0x00000001
            if (c.dw_cert_encoding_type & 0x0000_0001) != 0
                && !c.pb_cert_encoded.is_null()
                && c.cb_cert_encoded > 0
            {
                let slice = std::slice::from_raw_parts(c.pb_cert_encoded, c.cb_cert_encoded as usize);
                ders.push(slice.to_vec());
            }
            prev = ctx;
        }
        let _ = CertCloseStore(store, 0);
    }
    Ok(ders)
}
