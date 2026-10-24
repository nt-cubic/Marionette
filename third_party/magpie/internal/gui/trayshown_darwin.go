//go:build darwin && cgo && !nogui

package gui

/*
#cgo CFLAGS: -x objective-c
#cgo LDFLAGS: -framework Cocoa
#import <Cocoa/Cocoa.h>
#include <stdlib.h>

// AppKit keeps a status item's visibility in the app's defaults, by its
// autosave name ("NSStatusItem VisibleCC Item-0" on macOS 26, "NSStatusItem
// Visible Item-0" before), whoever set it: the user dragging it out, or the
// app's own setVisible:NO. The next item made under that name starts hidden.
static int mpHiddenKey(NSString *k, id v) {
	return [k hasPrefix:@"NSStatusItem Visible"] && [v respondsToSelector:@selector(boolValue)] && ![v boolValue];
}

// mpForgetHidden removes every status item visibility kept as hidden, and
// says how many it removed.
static int mpForgetHidden(void) {
	NSUserDefaults *d = [NSUserDefaults standardUserDefaults];
	int n = 0;
	for (NSString *k in [[d persistentDomainForName:[[NSBundle mainBundle] bundleIdentifier] ?: [[NSProcessInfo processInfo] processName]] allKeys]) {
		if (mpHiddenKey(k, [d objectForKey:k])) {
			[d removeObjectForKey:k];
			n++;
		}
	}
	if (n) [d synchronize];
	return n;
}

static int mpHiddenCount(void) {
	NSUserDefaults *d = [NSUserDefaults standardUserDefaults];
	int n = 0;
	for (NSString *k in [[d persistentDomainForName:[[NSBundle mainBundle] bundleIdentifier] ?: [[NSProcessInfo processInfo] processName]] allKeys]) {
		if (mpHiddenKey(k, [d objectForKey:k])) n++;
	}
	return n;
}

// After the blocks already queued on the main thread (Wails' setVisible:NO
// is one), so the hide is undone in the defaults only once it is made.
static void mpForgetHiddenLater(void) {
	dispatch_async(dispatch_get_main_queue(), ^{ mpForgetHidden(); });
}

static int mpFlag(const char *k) {
	return [[NSUserDefaults standardUserDefaults] boolForKey:[NSString stringWithUTF8String:k]];
}

static void mpSetFlag(const char *k) {
	NSUserDefaults *d = [NSUserDefaults standardUserDefaults];
	[d setBool:YES forKey:[NSString stringWithUTF8String:k]];
	[d synchronize];
}

// Waits, off the main thread, for what is queued on it to have run.
static void mpMainSettled(void) {
	dispatch_sync(dispatch_get_main_queue(), ^{});
}
*/
import "C"

import "unsafe"

// trayHidden is how many of this app's status items macOS keeps as hidden.
func trayHidden() int { return int(C.mpHiddenCount()) }

// forgetTrayHidden has the next status item start shown: it removes the
// hidden visibility AppKit keeps for it. How many it removed.
func forgetTrayHidden() int { return int(C.mpForgetHidden()) }

// forgetTrayHiddenLater is forgetTrayHidden once the main thread has run
// what is queued on it, the tray's Hide among them.
func forgetTrayHiddenLater() { C.mpForgetHiddenLater() }

// mainSettled waits for the main thread to run what is queued on it. Not
// on the main thread.
func mainSettled() { C.mpMainSettled() }

// trayBroughtBackKey marks, in the Mac's defaults (this computer's, never
// in a backup), that the hidden icon a handover left was brought back.
const trayBroughtBackKey = "magpie tray brought back after a handover"

// bringTrayBack, once per computer, brings back the menu bar icon a restart
// to update hid: from v0.1.1354 the magpie before hid its own item with
// setVisible:NO, which AppKit keeps for the app, so every later magpie
// started with no icon. A hide kept after that is the user's. Whether one
// stays hidden, to be left alone.
func bringTrayBack() (stillHidden bool) {
	k := C.CString(trayBroughtBackKey)
	defer C.free(unsafe.Pointer(k))
	if C.mpFlag(k) == 0 {
		forgetTrayHidden()
		C.mpSetFlag(k)
	}
	return trayHidden() > 0
}
