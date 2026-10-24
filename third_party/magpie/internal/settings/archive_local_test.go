package settings

import "testing"

// The archive on this computer's disk is this computer's own: sync, a
// Restore or a backup file never turns it on, or off, at another
// (CyberCaius on Discord; RequestArchive, the S3 archive's, is synced).
func TestRequestArchiveLocalIsThisComputers(t *testing.T) {
	from := Settings{RequestArchive: true, RequestArchiveLocal: true, RequestArchiveLocalMB: 64}
	from.KeepOwn(Settings{})
	if from.RequestArchiveLocal || from.RequestArchiveLocalMB != 0 {
		t.Fatalf("another computer's local archive came in: %+v", from)
	}
	if !from.RequestArchive {
		t.Fatal("the S3 archive's switch is synced")
	}
	from.KeepOwn(Settings{RequestArchiveLocal: true, RequestArchiveLocalMB: 8})
	if !from.RequestArchiveLocal || from.RequestArchiveLocalMB != 8 {
		t.Fatalf("this computer's own was lost: %+v", from)
	}
}
