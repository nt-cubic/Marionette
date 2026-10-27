//go:build !darwin

package dirent

func stats(string) (Listing, bool) { return nil, false }
