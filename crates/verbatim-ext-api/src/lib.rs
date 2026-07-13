//! Extension API (architecture section 7).
//!
//! The WIT-defined `verbatim:ext` worlds are the durable contract between
//! Verbatim and extensions, plus the guest SDK that app modules and global
//! extensions build against. The API grows only by porting real app modules
//! and add-ons: no host-API additions without a consumer.
//!
//! Skeleton only in M0; v0 of the API lands with milestone M5.
