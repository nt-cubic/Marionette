package davsync

import (
	"net/http"
	"strings"
	"testing"
	"time"
)

// The signer against AWS's own examples: the SigV4 test suite's (service
// "service") and the S3 documentation's, which sign a range, a key with $
// in it, a Date header, and a query with a name alone and one with values.
func TestSigV4Vectors(t *testing.T) {
	const empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
	suite := signer{id: "AKIDEXAMPLE", secret: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", region: "us-east-1", service: "service"}
	s3doc := signer{id: "AKIAIOSFODNN7EXAMPLE", secret: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", region: "us-east-1", service: "s3"}
	suiteAt := time.Date(2015, 8, 30, 12, 36, 0, 0, time.UTC)
	s3At := time.Date(2013, 5, 24, 0, 0, 0, 0, time.UTC)
	for _, c := range []struct {
		name    string
		s       signer
		at      time.Time
		method  string
		url     string
		headers map[string]string
		payload string
		want    string
	}{
		{"get-vanilla", suite, suiteAt, "GET", "https://example.amazonaws.com/", nil, empty,
			"AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"},
		{"get-vanilla-query-order-key-case", suite, suiteAt, "GET", "https://example.amazonaws.com/?Param2=value2&Param1=value1", nil, empty,
			"AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"},
		{"post-vanilla", suite, suiteAt, "POST", "https://example.amazonaws.com/", nil, empty,
			"AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"},
		{"s3 GET object", s3doc, s3At, "GET", "https://examplebucket.s3.amazonaws.com/test.txt",
			map[string]string{"Range": "bytes=0-9", "X-Amz-Content-Sha256": empty}, empty,
			"AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"},
		{"s3 PUT object", s3doc, s3At, "PUT", "https://examplebucket.s3.amazonaws.com/" + awsEscape("test$file.text", true),
			map[string]string{"Date": "Fri, 24 May 2013 00:00:00 GMT", "X-Amz-Storage-Class": "REDUCED_REDUNDANCY",
				"X-Amz-Content-Sha256": "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072"},
			sum([]byte("Welcome to Amazon S3.")),
			"AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=date;host;x-amz-content-sha256;x-amz-date;x-amz-storage-class, Signature=98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"},
		{"s3 GET lifecycle", s3doc, s3At, "GET", "https://examplebucket.s3.amazonaws.com/?lifecycle",
			map[string]string{"X-Amz-Content-Sha256": empty}, empty,
			"AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"},
		{"s3 list objects", s3doc, s3At, "GET", "https://examplebucket.s3.amazonaws.com/?max-keys=2&prefix=J",
			map[string]string{"X-Amz-Content-Sha256": empty}, empty,
			"AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"},
	} {
		req, err := http.NewRequest(c.method, c.url, nil)
		if err != nil {
			t.Fatal(err)
		}
		for k, v := range c.headers {
			req.Header.Set(k, v)
		}
		c.s.sign(req, c.payload, c.at)
		if got := req.Header.Get("Authorization"); got != c.want {
			t.Errorf("%s:\n got %s\nwant %s", c.name, got, c.want)
		}
		if !strings.HasPrefix(req.Header.Get("X-Amz-Date"), c.at.Format("20060102T")) {
			t.Errorf("%s: X-Amz-Date %q", c.name, req.Header.Get("X-Amz-Date"))
		}
	}
	if got := awsEscape("a b/c~d+e$é", true); got != "a%20b/c~d%2Be%24%C3%A9" {
		t.Errorf("escaped: %s", got)
	}
}
