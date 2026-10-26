#!/usr/bin/env ruby
# frozen_string_literal: true

# Writes THIRD-PARTY-LICENSES.txt: the licences of every crate compiled into
# the release binaries, which their licences require to be passed on.
#
# The crate list comes from `cargo tree` for each release target (normal
# dependencies only: build tools and test-only crates are not shipped), the
# licence texts from the crates' sources as `cargo metadata` locates them.
# Identical texts are printed once, followed by the crates they apply to.
#
#   ruby scripts/third-party-licenses.rb            # rewrite the file
#   ruby scripts/third-party-licenses.rb --check    # fail if it is out of date
#
# Run it after changing dependencies; the release workflow runs --check.

require 'json'
require 'open3'

ROOT = File.expand_path('..', __dir__)
OUTPUT = File.join(ROOT, 'THIRD-PARTY-LICENSES.txt')
REPO = 'https://github.com/msetkowski/contact-sheet'

# Keep in sync with the build matrix in .github/workflows/release.yml.
TARGETS = %w[
  aarch64-apple-darwin
  x86_64-apple-darwin
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-musl
  x86_64-pc-windows-msvc
].freeze

LICENSE_FILE = /\A(licen[cs]e|copying|copyright|notice)/i.freeze

# Licence chosen for a crate that offers several, when it ships no licence
# file and a standard text has to be supplied.
PREFERRED = %w[Apache-2.0 MIT BSD-3-Clause Zlib].freeze

MIT_TEXT = <<~TEXT
  Copyright (c) %<holders>s

  Permission is hereby granted, free of charge, to any person obtaining a copy
  of this software and associated documentation files (the "Software"), to deal
  in the Software without restriction, including without limitation the rights
  to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
  copies of the Software, and to permit persons to whom the Software is
  furnished to do so, subject to the following conditions:

  The above copyright notice and this permission notice shall be included in all
  copies or substantial portions of the Software.

  THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
  IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
  FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
  AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
  LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
  OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
  SOFTWARE.
TEXT

BSD3_TEXT = <<~TEXT
  Copyright (c) %<holders>s
  All rights reserved.

  Redistribution and use in source and binary forms, with or without
  modification, are permitted provided that the following conditions are met:

  1. Redistributions of source code must retain the above copyright notice, this
     list of conditions and the following disclaimer.

  2. Redistributions in binary form must reproduce the above copyright notice,
     this list of conditions and the following disclaimer in the documentation
     and/or other materials provided with the distribution.

  3. Neither the name of the copyright holder nor the names of its
     contributors may be used to endorse or promote products derived from
     this software without specific prior written permission.

  THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
  AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
  IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
  DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
  FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
  DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
  SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
  CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
  OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
  OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
TEXT

def run(*cmd)
  # Plain output to parse, even where CARGO_TERM_COLOR=always (as in CI).
  env = { 'CARGO_TERM_COLOR' => 'never' }
  out, err, status = Open3.capture3(env, *cmd, chdir: ROOT)
  abort("#{cmd.join(' ')} failed:\n#{err}") unless status.success?
  out
end

# "name vX.Y.Z" of every crate in the release binaries, without our own.
def shipped_crates
  TARGETS.flat_map do |target|
    run('cargo', 'tree', '--locked', '-e', 'normal', '--target', target,
        '--prefix', 'none', '-f', '{p}')
      .lines.map { |l| l.sub(/ \(.*\)$/, '').strip }
  end.uniq.reject { |c| c.empty? || c.start_with?('contact-sheet ') }
end

def packages
  meta = JSON.parse(run('cargo', 'metadata', '--locked', '--format-version', '1'))
  meta['packages'].map { |p| ["#{p['name']} v#{p['version']}", p] }.to_h
end

def holders(pkg)
  authors = pkg['authors'].map { |a| a.sub(/\s*<.*>/, '') }
  authors.empty? ? "the #{pkg['name']} developers" : authors.join(', ')
end

# Licence files shipped with the crate, or a standard text when it has none.
def license_texts(pkg, apache_text)
  dir = File.dirname(pkg['manifest_path'])
  files = Dir.children(dir).select { |f| f =~ LICENSE_FILE && File.file?(File.join(dir, f)) }.sort
  return files.map { |f| File.read(File.join(dir, f), encoding: 'UTF-8').scrub } unless files.empty?

  offered = pkg['license'].to_s.tr('/', ' OR ').split(/\s+OR\s+/).map { |l| l.delete('()').strip }
  chosen = PREFERRED.find { |l| offered.include?(l) }
  case chosen
  when 'Apache-2.0' then [apache_text]
  when 'MIT' then [format(MIT_TEXT, holders: holders(pkg))]
  when 'BSD-3-Clause' then [format(BSD3_TEXT, holders: holders(pkg))]
  else abort("#{pkg['name']} has no licence file and no standard text for #{pkg['license']}")
  end
end

def normalize(text)
  text.gsub(/\r\n?/, "\n").strip.gsub(/[ \t]+$/, '')
end

def copyleft?(pkg)
  pkg['license'].to_s =~ /GPL/ && pkg['license'] !~ /\bOR\b|\//
end

def lgpl_section(pkg)
  name = pkg['name']
  version = pkg['version']
  <<~TEXT
    #{name} #{version} – #{pkg['license']}

    contact-sheet contains #{name}, which is licensed under the GNU Lesser General
    Public License (#{pkg['license']}); its full text is in the licence texts
    below.

    Source code: the complete source of #{name} #{version} is available from
    https://crates.io/crates/#{name}/#{version}
    (archive: https://static.crates.io/crates/#{name}/#{name}-#{version}.crate)
    and from #{pkg['repository']}.
    For at least three years after each contact-sheet release we will also
    provide it on request: open an issue at #{REPO}/issues.

    Relinking: contact-sheet is open source (Apache-2.0), and its source with the
    exact dependency versions (Cargo.lock) is available at #{REPO}. To use a
    modified #{name}, build contact-sheet from source with a [patch.crates-io]
    entry in Cargo.toml that points to your version of #{name}.
  TEXT
end

def generate
  crates = shipped_crates
  pkgs = packages
  apache_text = File.read(File.join(ROOT, 'LICENSE'))

  shipped = crates.map { |c| pkgs.fetch(c) { abort("#{c} not found in cargo metadata") } }
                  .sort_by { |p| [p['name'], p['version']] }

  # Group crates by identical licence text.
  groups = Hash.new { |h, k| h[k] = [] }
  shipped.each do |pkg|
    license_texts(pkg, apache_text).each { |t| groups[normalize(t)] << pkg }
  end
  groups.each_value { |list| list.uniq! }

  rule = '=' * 79
  out = +''
  out << <<~HEADER
    THIRD-PARTY SOFTWARE NOTICES AND LICENSES

    contact-sheet (#{REPO}) is licensed under the Apache License 2.0
    (see LICENSE). Its binaries also contain the third-party software listed
    below. The DejaVu Sans font embedded for the labels is covered by
    LICENSE-DejaVu.txt; the embedded ICC profiles (FOGRA39L from colord and sRGB
    from Compact ICC Profiles) are in the public domain (CC0 1.0).

    This file is generated by scripts/third-party-licenses.rb.

  HEADER

  shipped.select { |p| copyleft?(p) }.each do |pkg|
    out << "#{rule}\n#{lgpl_section(pkg)}\n"
  end

  out << "#{rule}\nCOMPONENTS\n\n"
  width = shipped.map { |p| "#{p['name']} #{p['version']}".size }.max
  shipped.each do |p|
    out << "#{"#{p['name']} #{p['version']}".ljust(width)}  #{p['license']}\n"
  end
  out << "\n"

  out << "#{rule}\nLICENSE TEXTS\n"
  groups.sort_by { |_, list| list.map { |p| p['name'] }.min }.each do |text, list|
    names = list.map { |p| "#{p['name']} #{p['version']}" }.join(', ')
    out << "\n#{'-' * 79}\nUsed by: #{names}\n#{'-' * 79}\n\n#{text}\n"
  end
  out
end

content = generate
if ARGV.include?('--check')
  current = File.exist?(OUTPUT) ? File.read(OUTPUT, encoding: 'UTF-8') : ''
  if current == content
    puts 'THIRD-PARTY-LICENSES.txt is up to date'
  else
    abort('THIRD-PARTY-LICENSES.txt is out of date; run: ruby scripts/third-party-licenses.rb')
  end
else
  File.write(OUTPUT, content)
  puts "Wrote #{OUTPUT} (#{content.lines.size} lines)"
end
