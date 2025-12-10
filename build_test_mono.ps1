param(
  [Parameter(Mandatory=$true)] [string] $Format,
  [Parameter(Mandatory=$true)] [int] $Bits,
  [Parameter(Mandatory=$true)] [string] $Endianness,
  [Parameter(Mandatory=$true)] [string] $Level,
  [Parameter(Mandatory=$true)] [string] $InputFile
)

# Adjust output sample size for ffplay when 20-bit is requested
$outBits = $Bits
if ($Bits -eq 20) {
  $outBits = 24
}

# Build the Rust project
cargo build

# Pipe input file into dsd2dxd and stream to ffplay
Get-Content -AsByteStream -Raw $InputFile | .\target\debug\dsd2dxd -c 1 -f $Format -b $outBits -e $Endianness --level=$Level | ffplay -f "s${outBits}le" -ar 352.8k -ch_layout mono -i -
