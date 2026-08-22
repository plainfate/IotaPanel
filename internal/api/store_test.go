// SPDX-License-Identifier: Apache-2.0
//
// Copyright (C) 2026 plainfate <https://github.com/plainfate>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package api

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"testing"
)

// buildPluginPackage 构造内存中的 gzip+tar 插件包：entries 为 相对路径 -> 内容。
func buildPluginPackage(t *testing.T, entries map[string]string) []byte {
	t.Helper()
	var buf bytes.Buffer
	gz := gzip.NewWriter(&buf)
	tw := tar.NewWriter(gz)
	for name, content := range entries {
		if err := tw.WriteHeader(&tar.Header{Name: name, Mode: 0o644, Size: int64(len(content)), Typeflag: tar.TypeReg}); err != nil {
			t.Fatal(err)
		}
		if _, err := tw.Write([]byte(content)); err != nil {
			t.Fatal(err)
		}
	}
	if err := tw.Close(); err != nil {
		t.Fatal(err)
	}
	if err := gz.Close(); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

func TestUnpackPluginPackageValid(t *testing.T) {
	data := buildPluginPackage(t, map[string]string{
		"demo/manifest.yaml": "name: demo\n",
		"demo/bin/run":       "#!/bin/sh\n",
	})
	name, files, err := unpackPluginPackage(data)
	if err != nil {
		t.Fatalf("valid package rejected: %v", err)
	}
	if name != "demo" {
		t.Fatalf("top dir = %q, want demo", name)
	}
	if _, ok := files["manifest.yaml"]; !ok {
		t.Fatal("manifest.yaml missing from extracted files")
	}
}

func TestUnpackPluginPackageRejectsTraversal(t *testing.T) {
	data := buildPluginPackage(t, map[string]string{
		"../../etc/cron.d/evil": "x",
	})
	if _, _, err := unpackPluginPackage(data); err == nil {
		t.Fatal("path traversal accepted")
	}
}

func TestUnpackPluginPackageRejectsMultipleTopDirs(t *testing.T) {
	data := buildPluginPackage(t, map[string]string{
		"a/manifest.yaml": "name: a\n",
		"b/manifest.yaml": "name: b\n",
	})
	if _, _, err := unpackPluginPackage(data); err == nil {
		t.Fatal("multiple top-level dirs accepted")
	}
}

func TestUnpackPluginPackageRequiresManifest(t *testing.T) {
	data := buildPluginPackage(t, map[string]string{
		"demo/readme.txt": "hi",
	})
	if _, _, err := unpackPluginPackage(data); err == nil {
		t.Fatal("package without manifest.yaml accepted")
	}
}

func TestUnpackPluginPackageRejectsNonGzip(t *testing.T) {
	if _, _, err := unpackPluginPackage([]byte("this is not gzip")); err == nil {
		t.Fatal("non-gzip payload accepted")
	}
}
