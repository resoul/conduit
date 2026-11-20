//! TLS error types and alert definitions.

use std::fmt;
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AlertDescription {
    CloseNotify = 0,
    UnexpectedMessage = 10,
    BadRecordMac = 20,
    RecordOverflow = 22,
    HandshakeFailure = 40,
    BadCertificate = 42,
    UnsupportedCertificate = 43,
    CertificateRevoked = 44,
    CertificateExpired = 45,
    CertificateUnknown = 46,
    IllegalParameter = 47,
    UnknownCa = 48,
    AccessDenied = 49,
    DecodeError = 50,
    DecryptError = 51,
    ProtocolVersion = 70,
    InsufficientSecurity = 71,
    InternalError = 80,
    InappropriateFallback = 86,
    UserCanceled = 90,
    MissingExtension = 109,
    UnsupportedExtension = 110,
    UnrecognizedName = 112,
    BadCertificateStatusResponse = 113,
    UnknownPskIdentity = 115,
    CertificateRequired = 116,
    NoApplicationProtocol = 120,
    Unknown(u8),
}

impl AlertDescription {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::CloseNotify,
            10 => Self::UnexpectedMessage,
            20 => Self::BadRecordMac,
            22 => Self::RecordOverflow,
            40 => Self::HandshakeFailure,
            42 => Self::BadCertificate,
            43 => Self::UnsupportedCertificate,
            44 => Self::CertificateRevoked,
            45 => Self::CertificateExpired,
            46 => Self::CertificateUnknown,
            47 => Self::IllegalParameter,
            48 => Self::UnknownCa,
            49 => Self::AccessDenied,
            50 => Self::DecodeError,
            51 => Self::DecryptError,
            70 => Self::ProtocolVersion,
            71 => Self::InsufficientSecurity,
            80 => Self::InternalError,
            86 => Self::InappropriateFallback,
            90 => Self::UserCanceled,
            109 => Self::MissingExtension,
            110 => Self::UnsupportedExtension,
            112 => Self::UnrecognizedName,
            113 => Self::BadCertificateStatusResponse,
            115 => Self::UnknownPskIdentity,
            116 => Self::CertificateRequired,
            120 => Self::NoApplicationProtocol,
            other => Self::Unknown(other),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CloseNotify => "close_notify",
            Self::UnexpectedMessage => "unexpected_message",
            Self::BadRecordMac => "bad_record_mac",
            Self::RecordOverflow => "record_overflow",
            Self::HandshakeFailure => "handshake_failure",
            Self::BadCertificate => "bad_certificate",
            Self::UnsupportedCertificate => "unsupported_certificate",
            Self::CertificateRevoked => "certificate_revoked",
            Self::CertificateExpired => "certificate_expired",
            Self::CertificateUnknown => "certificate_unknown",
            Self::IllegalParameter => "illegal_parameter",
            Self::UnknownCa => "unknown_ca",
            Self::AccessDenied => "access_denied",
            Self::DecodeError => "decode_error",
            Self::DecryptError => "decrypt_error",
            Self::ProtocolVersion => "protocol_version",
            Self::InsufficientSecurity => "insufficient_security",
            Self::InternalError => "internal_error",
            Self::InappropriateFallback => "inappropriate_fallback",
            Self::UserCanceled => "user_canceled",
            Self::MissingExtension => "missing_extension",
            Self::UnsupportedExtension => "unsupported_extension",
            Self::UnrecognizedName => "unrecognized_name",
            Self::BadCertificateStatusResponse => "bad_certificate_status_response",
            Self::UnknownPskIdentity => "unknown_psk_identity",
            Self::CertificateRequired => "certificate_required",
            Self::NoApplicationProtocol => "no_application_protocol",
            Self::Unknown(_) => "unknown_alert",
        }
    }
}

#[derive(Debug)]
pub enum TlsError {
    Io(io::Error),
    UnexpectedEof,
    DecodeError(&'static str),
    ProtocolError(String),
    RemoteAlert {
        level: u8,
        description: AlertDescription,
    },
    CertificateError(String),
    CryptoError(&'static str),
    NoTrustAnchors,
    Timeout,
}

impl TlsError {
    pub fn is_certificate_error(&self) -> bool {
        matches!(
            self,
            Self::CertificateError(_)
                | Self::NoTrustAnchors
                | Self::RemoteAlert {
                    description: AlertDescription::BadCertificate
                        | AlertDescription::UnsupportedCertificate
                        | AlertDescription::CertificateRevoked
                        | AlertDescription::CertificateExpired
                        | AlertDescription::CertificateUnknown
                        | AlertDescription::UnknownCa,
                    ..
                }
        )
    }
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "TLS I/O error: {e}"),
            Self::UnexpectedEof => write!(f, "TLS connection closed unexpectedly (missing close_notify)"),
            Self::DecodeError(msg) => write!(f, "TLS decode error: {msg}"),
            Self::ProtocolError(msg) => write!(f, "TLS protocol error: {msg}"),
            Self::RemoteAlert { level, description } => {
                write!(f, "TLS remote alert (level {level}): {}", description.as_str())
            }
            Self::CertificateError(msg) => write!(f, "TLS certificate error: {msg}"),
            Self::CryptoError(msg) => write!(f, "TLS crypto error: {msg}"),
            Self::NoTrustAnchors => write!(
                f,
                "no trust anchors found, set sslrootcert/CONDUIT_CA_FILE"
            ),
            Self::Timeout => write!(f, "TLS operation timed out"),
        }
    }
}

impl std::error::Error for TlsError {}

impl From<io::Error> for TlsError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::UnexpectedEof => Self::UnexpectedEof,
            io::ErrorKind::TimedOut => Self::Timeout,
            _ => Self::Io(e),
        }
    }
}

impl From<TlsError> for String {
    fn from(e: TlsError) -> Self {
        e.to_string()
    }
}
