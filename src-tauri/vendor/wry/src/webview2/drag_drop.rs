// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

// A silly implementation of file drop handling for Windows!

use crate::DragDropEvent;

use std::{
  cell::UnsafeCell,
  ffi::OsString,
  os::{raw::c_void, windows::ffi::OsStringExt},
  path::PathBuf,
  ptr,
  rc::Rc,
};

use windows::{
  core::{implement, BOOL},
  Win32::{
    Foundation::{DRAGDROP_E_INVALIDHWND, HWND, LPARAM, POINT, POINTL},
    Graphics::Gdi::ScreenToClient,
    System::{
      Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL},
      Memory::{GlobalLock, GlobalSize, GlobalUnlock},
      Ole::{
        IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
        CF_HDROP, CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
      },
      SystemServices::MODIFIERKEYS_FLAGS,
    },
    UI::{
      Shell::{DragFinish, DragQueryFileW, HDROP},
      WindowsAndMessaging::EnumChildWindows,
    },
  },
};

use crate::marionette_drag::{
  emit_text_drag, KIND_DROP, KIND_ENTER, KIND_LEAVE, KIND_OVER,
};

#[derive(Default)]
pub(crate) struct DragDropController {
  drop_targets: Vec<IDropTarget>,
}

impl DragDropController {
  #[inline]
  pub(crate) fn new(hwnd: HWND, handler: Box<dyn Fn(DragDropEvent) -> bool>) -> Self {
    let mut controller = DragDropController::default();

    let handler = Rc::new(handler);

    // Enumerate child windows to find the WebView2 "window" and override!
    {
      let mut callback = |hwnd| controller.inject_in_hwnd(hwnd, handler.clone());
      let mut trait_obj: &mut dyn FnMut(HWND) -> bool = &mut callback;
      let closure_pointer_pointer: *mut c_void = unsafe { std::mem::transmute(&mut trait_obj) };
      let lparam = LPARAM(closure_pointer_pointer as _);
      unsafe extern "system" fn enumerate_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let closure = &mut *(lparam.0 as *mut c_void as *mut &mut dyn FnMut(HWND) -> bool);
        closure(hwnd).into()
      }
      let _ = unsafe { EnumChildWindows(Some(hwnd), Some(enumerate_callback), lparam) };
    }

    controller
  }

  #[inline]
  fn inject_in_hwnd(&mut self, hwnd: HWND, handler: Rc<dyn Fn(DragDropEvent) -> bool>) -> bool {
    let drag_drop_target: IDropTarget = DragDropTarget::new(hwnd, handler).into();
    if unsafe { RevokeDragDrop(hwnd) } != Err(DRAGDROP_E_INVALIDHWND.into())
      && unsafe { RegisterDragDrop(hwnd, &drag_drop_target) }.is_ok()
    {
      self.drop_targets.push(drag_drop_target);
    }

    true
  }
}

#[implement(IDropTarget)]
pub struct DragDropTarget {
  hwnd: HWND,
  listener: Rc<dyn Fn(DragDropEvent) -> bool>,
  cursor_effect: UnsafeCell<DROPEFFECT>,
  enter_is_valid: UnsafeCell<bool>, /* If the currently hovered item is not valid there must not be any `HoveredFileCancelled` emitted */
  // Marionette patch: active text-only drag (no CF_HDROP, text captured at Enter).
  text_drag: UnsafeCell<Option<String>>,
}

impl DragDropTarget {
  pub fn new(hwnd: HWND, listener: Rc<dyn Fn(DragDropEvent) -> bool>) -> DragDropTarget {
    Self {
      hwnd,
      listener,
      cursor_effect: DROPEFFECT_NONE.into(),
      enter_is_valid: false.into(),
      text_drag: None.into(),
    }
  }

  unsafe fn iterate_filenames<F>(
    data_obj: windows_core::Ref<'_, IDataObject>,
    mut callback: F,
  ) -> Option<HDROP>
  where
    F: FnMut(PathBuf),
  {
    let drop_format = FORMATETC {
      cfFormat: CF_HDROP.0,
      ptd: ptr::null_mut(),
      dwAspect: DVASPECT_CONTENT.0,
      lindex: -1,
      tymed: TYMED_HGLOBAL.0 as u32,
    };

    match data_obj
      .as_ref()
      .expect("Received null IDataObject")
      .GetData(&drop_format)
    {
      Ok(medium) => {
        let hdrop = HDROP(medium.u.hGlobal.0 as _);

        // The second parameter (0xFFFFFFFF) instructs the function to return the item count
        let item_count = DragQueryFileW(hdrop, 0xFFFFFFFF, None);

        for i in 0..item_count {
          // Get the length of the path string NOT including the terminating null character.
          // Previously, this was using a fixed size array of MAX_PATH length, but the
          // Windows API allows longer paths under certain circumstances.
          let character_count = DragQueryFileW(hdrop, i, None) as usize;

          // Fill path_buf with the null-terminated file name
          let str_len = character_count + 1;
          let mut path_buf = vec![0; str_len];
          DragQueryFileW(hdrop, i, Some(&mut path_buf));
          callback(OsString::from_wide(&path_buf[0..character_count]).into());
        }

        Some(hdrop)
      }
      Err(_error) => {
        #[cfg(feature = "tracing")]
        tracing::warn!(
          "{}",
          match _error.code() {
            windows::Win32::Foundation::DV_E_FORMATETC => {
              // If the dropped item is not a file this error will occur.
              // In this case it is OK to return without taking further action.
              "Error occurred while processing dropped/hovered item: item is not a file."
            }
            _ => "Unexpected error occurred while processing dropped/hovered item.",
          }
        );
        None
      }
    }
  }

  /// Marionette patch: read the drag payload as text (`CF_UNICODETEXT`) for
  /// non-file drags — selected text, browser links, image URLs. None when the
  /// source offers no text (or renders it lazily past the Enter query).
  unsafe fn read_drag_text(data_obj: &windows_core::Ref<'_, IDataObject>) -> Option<String> {
    const MAX_TEXT_CHARS: usize = 1_000_000;

    let data_obj = data_obj.as_ref()?;
    let text_format = FORMATETC {
      cfFormat: CF_UNICODETEXT.0,
      ptd: ptr::null_mut(),
      dwAspect: DVASPECT_CONTENT.0,
      lindex: -1,
      tymed: TYMED_HGLOBAL.0 as u32,
    };

    let mut medium = match data_obj.GetData(&text_format) {
      Ok(medium) => medium,
      Err(_) => return None,
    };

    let hglobal = medium.u.hGlobal;
    let lock = GlobalLock(hglobal);
    if lock.is_null() {
      ReleaseStgMedium(&mut medium);
      return None;
    }

    let byte_len = GlobalSize(hglobal);
    let u16_len = (byte_len / 2).min(MAX_TEXT_CHARS);
    let slice = std::slice::from_raw_parts(lock as *const u16, u16_len);
    let nul = slice.iter().position(|&c| c == 0).unwrap_or(u16_len);
    let text = String::from_utf16_lossy(&slice[..nul]);
    let _ = GlobalUnlock(hglobal);
    ReleaseStgMedium(&mut medium);

    if text.trim().is_empty() {
      None
    } else {
      Some(text)
    }
  }
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for DragDropTarget_Impl {
  fn DragEnter(
    &self,
    pDataObj: windows_core::Ref<'_, IDataObject>,
    _grfKeyState: MODIFIERKEYS_FLAGS,
    pt: &POINTL,
    pdwEffect: *mut DROPEFFECT,
  ) -> windows::core::Result<()> {
    let mut pt = POINT { x: pt.x, y: pt.y };
    let _ = unsafe { ScreenToClient(self.hwnd, &mut pt) };

    // Marionette patch: probe the text payload first (borrow); stock code
    // consumes `pDataObj` by value right below.
    let text_payload = unsafe { DragDropTarget::read_drag_text(&pDataObj) };

    let mut paths = Vec::new();
    let hdrop = unsafe { DragDropTarget::iterate_filenames(pDataObj, |path| paths.push(path)) };

    let enter_is_valid = hdrop.is_some();

    if !enter_is_valid {
      // Marionette patch: no CF_HDROP on this drag. If it carries text
      // (browser selection/link/image URL) forward it through the text hook
      // and keep the stock file-drop event stream untouched.
      if let Some(text) = text_payload {
        if emit_text_drag(
          KIND_ENTER,
          &text,
          pt.x as _,
          pt.y as _,
          self.hwnd.0 as isize,
        ) {
          unsafe {
            *self.enter_is_valid.get() = false;
            *self.text_drag.get() = Some(text);
            *pdwEffect = DROPEFFECT_COPY;
            *self.cursor_effect.get() = DROPEFFECT_COPY;
          }
        }
      }
      return Ok(());
    };

    unsafe {
      *self.enter_is_valid.get() = enter_is_valid;
    }

    (self.listener)(DragDropEvent::Enter {
      paths,
      position: (pt.x as _, pt.y as _),
    });

    let cursor_effect = if enter_is_valid {
      DROPEFFECT_COPY
    } else {
      DROPEFFECT_NONE
    };

    unsafe {
      *pdwEffect = cursor_effect;
      *self.cursor_effect.get() = cursor_effect;
    }

    Ok(())
  }

  fn DragOver(
    &self,
    _grfKeyState: MODIFIERKEYS_FLAGS,
    pt: &POINTL,
    pdwEffect: *mut DROPEFFECT,
  ) -> windows::core::Result<()> {
    // Marionette patch: text-only drag — mirror the stock Over flow via the hook.
    unsafe {
      if let Some(text) = &*self.text_drag.get() {
        let mut pt = POINT { x: pt.x, y: pt.y };
        let _ = ScreenToClient(self.hwnd, &mut pt);
        emit_text_drag(
          KIND_OVER,
          text,
          pt.x as _,
          pt.y as _,
          self.hwnd.0 as isize,
        );
        *pdwEffect = DROPEFFECT_COPY;
        return Ok(());
      }
    }

    if unsafe { *self.enter_is_valid.get() } {
      let mut pt = POINT { x: pt.x, y: pt.y };
      let _ = unsafe { ScreenToClient(self.hwnd, &mut pt) };
      (self.listener)(DragDropEvent::Over {
        position: (pt.x as _, pt.y as _),
      });
    }

    unsafe { *pdwEffect = *self.cursor_effect.get() };
    Ok(())
  }

  fn DragLeave(&self) -> windows::core::Result<()> {
    // Marionette patch: text-only drag cancelled.
    unsafe {
      if (*self.text_drag.get()).take().is_some() {
        emit_text_drag(KIND_LEAVE, "", 0.0, 0.0, self.hwnd.0 as isize);
        return Ok(());
      }
    }

    if unsafe { *self.enter_is_valid.get() } {
      (self.listener)(DragDropEvent::Leave);
    }
    Ok(())
  }

  fn Drop(
    &self,
    pDataObj: windows_core::Ref<'_, IDataObject>,
    _grfKeyState: MODIFIERKEYS_FLAGS,
    pt: &POINTL,
    _pdwEffect: *mut DROPEFFECT,
  ) -> windows::core::Result<()> {
    // Marionette patch: deliver text-only drops through the hook. The payload
    // was captured at Enter (OLE data objects are immutable snapshots).
    unsafe {
      if let Some(text) = (*self.text_drag.get()).take() {
        let mut pt = POINT { x: pt.x, y: pt.y };
        let _ = ScreenToClient(self.hwnd, &mut pt);
        emit_text_drag(
          KIND_DROP,
          &text,
          pt.x as _,
          pt.y as _,
          self.hwnd.0 as isize,
        );
        *_pdwEffect = DROPEFFECT_COPY;
        return Ok(());
      }
    }

    if unsafe { *self.enter_is_valid.get() } {
      let mut pt = POINT { x: pt.x, y: pt.y };
      let _ = unsafe { ScreenToClient(self.hwnd, &mut pt) };

      let mut paths = Vec::new();
      let hdrop = unsafe { DragDropTarget::iterate_filenames(pDataObj, |path| paths.push(path)) };
      (self.listener)(DragDropEvent::Drop {
        paths,
        position: (pt.x as _, pt.y as _),
      });

      if let Some(hdrop) = hdrop {
        unsafe { DragFinish(hdrop) };
      }
    }

    Ok(())
  }
}
