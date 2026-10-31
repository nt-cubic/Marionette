package coreml

// Devices are the devices a plan places operations on, in gmlPlan's order.
var Devices = [3]string{"Neural Engine", "GPU", "CPU"}

// Most is the device of shares with the largest, "" with none.
func Most(shares [3]float64) string {
	best := -1
	for i, s := range shares {
		if s > 0 && (best < 0 || s > shares[best]) {
			best = i
		}
	}
	if best < 0 {
		return ""
	}
	return Devices[best]
}
