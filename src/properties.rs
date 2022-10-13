use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    num::NonZeroU64,
    ops::Deref,
    ptr::NonNull,
};

use crate::{sys, Error, JackStr, Result, Uuid};

/// A type for C-owned property keys.
///
/// Only used for the built-in property keys in [`crate::metadata`].
#[derive(Debug)]
pub struct PropertyKey(pub(crate) *const std::os::raw::c_char);

unsafe impl Send for PropertyKey {}
unsafe impl Sync for PropertyKey {}

impl AsRef<CStr> for PropertyKey {
    fn as_ref(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.0) }
    }
}

///  Get a property on subject.
///
/// Returns a tuple of `(value, type)` where:
///
/// - `value` - Set to the value of the property.
/// - `type` - The type of the property if set, or NULL. See [`Property`] for the discussion
///   of types.
///
/// # Parameters
///
/// - `subject` - The subject to get the property from.
/// - `key` - The key of the property.
#[doc(alias = "jack_get_property")]
pub fn property(subject: Uuid, key: impl AsRef<CStr>) -> Result<(CString, Option<CString>)> {
    let mut value = std::ptr::null_mut();
    let mut type_ = std::ptr::null_mut();
    Error::check_ret(unsafe {
        sys::weak_library()?.jack_get_property(
            subject.0.get(),
            key.as_ref().as_ptr(),
            &mut value,
            &mut type_,
        )
    })?;
    let value = unsafe { JackStr::from_raw_unchecked(value) };
    let type_ = NonNull::new(type_).map(JackStr);
    unsafe { Ok((value.as_cstring(), type_.map(|t| t.as_cstring()))) }
}

/// Get a description of subject.
#[doc(alias = "jack_get_properties")]
pub fn properties(subject: Uuid) -> Result<PropertyDescription> {
    let mut desc = MaybeUninit::uninit();
    Error::check_ret(unsafe {
        sys::weak_library()?.jack_get_properties(subject.0.get(), desc.as_mut_ptr())
    })?;
    Ok(PropertyDescription(unsafe { desc.assume_init() }))
}

/// A single property (key:value pair).
///
/// Although there is no semantics imposed on metadata keys and values, it is much less useful to
/// use it to associate highly structured data with a port (or client), since this then implies the
/// need for some (presumably library-based) code to parse the structure and be able to use it.
///
/// The real goal of the metadata API is to be able to tag ports (and clients) with small amounts
/// of data that is outside of the core JACK API but nevertheless useful.
#[doc(alias = "jack_property_t")]
#[repr(transparent)]
#[derive(Debug)]
pub struct Property(sys::jack_property_t);

unsafe impl Send for Property {}
unsafe impl Sync for Property {}

impl Property {
    /// Returns the key of this property (URI string).
    pub fn key(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.0.key) }
    }
    /// Returns the property value (null-terminated string).
    pub fn data(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.0.data) }
    }
    /// Returns the type of property data, either a MIME type or URI.
    ///
    /// If type is `None` or empty, the data is assumed to be a UTF-8 encoded string
    /// (`text/plain`). The data is a null-terminated string regardless of type, so values can
    /// always be copied, but clients should not try to interpret values of an unknown type.
    ///
    /// Example values:
    /// - `image/png;base64` (base64 encoded PNG image)
    /// - [`http://www.w3.org/2001/XMLSchema#int`] (integer)
    ///
    /// Official types are preferred, but clients may use any syntactically valid MIME type (which
    /// start with a type and slash, like `"text/..."`). If a URI type is used, it must be a complete
    /// absolute URI (which start with a scheme and colon, like `"http:"`).
    pub fn type_(&self) -> Option<&CStr> {
        if self.0.type_.is_null() {
            return None;
        }
        Some(unsafe { CStr::from_ptr(self.0.type_) })
    }
}

/// A description of a subject (a set of properties).
///
/// The property list can be accessed through the [`Deref`] implementation.
#[doc(alias = "jack_description_t")]
#[repr(transparent)]
#[derive(Debug)]
pub struct PropertyDescription(sys::jack_description_t);

unsafe impl Send for PropertyDescription {}
unsafe impl Sync for PropertyDescription {}

impl PropertyDescription {
    /// Returns the current subject of this property list.
    #[inline]
    pub fn subject(&self) -> Uuid {
        Uuid(unsafe { NonZeroU64::new_unchecked(self.0.subject) })
    }
}

impl Deref for PropertyDescription {
    type Target = [Property];

    fn deref(&self) -> &Self::Target {
        unsafe {
            std::slice::from_raw_parts(self.0.properties as *const _, self.0.property_cnt as usize)
        }
    }
}

impl Drop for PropertyDescription {
    fn drop(&mut self) {
        unsafe {
            sys::library().jack_free_description(&mut self.0, 0);
        }
    }
}

#[doc(alias = "jack_get_all_properties")]
/// Get descriptions for all subjects with metadata.
pub fn all_properties() -> Result<PropertyDescriptionList> {
    let mut descs = MaybeUninit::uninit();
    let count = unsafe { sys::weak_library()?.jack_get_all_properties(descs.as_mut_ptr()) };
    Error::check_ret(count)?;
    Ok(PropertyDescriptionList {
        ptr: unsafe { descs.assume_init() },
        count: count as u32,
    })
}

/// A list of [`PropertyDescription`]s.
///
/// The descriptions can be accessed through the [`Deref`] implementation.
#[derive(Debug)]
pub struct PropertyDescriptionList {
    ptr: *mut sys::jack_description_t,
    count: u32,
}

unsafe impl Send for PropertyDescriptionList {}
unsafe impl Sync for PropertyDescriptionList {}

impl Deref for PropertyDescriptionList {
    type Target = [PropertyDescription];

    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.ptr as *const _, self.count as usize) }
    }
}

impl Drop for PropertyDescriptionList {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                for index in 0..self.count {
                    sys::library().jack_free_description(self.ptr.add(index as usize), 0);
                }
                sys::library().jack_free(self.ptr as *mut _);
            }
        }
    }
}
