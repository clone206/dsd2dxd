# Ensure dsd2dxd is installed to cargo bin
cargo install --path .

# Play back various resample rates at 48k multiples
dsd2dxd -r 96000   .\test\1kHz_stereo_p.dsf   | ffplay -f s24le -ar 96k  -ch_layout stereo -i -
dsd2dxd -r 192000  .\test\1kHz_stereo_p.dsf   | ffplay -f s24le -ar 192k -ch_layout stereo -i -
dsd2dxd -r 384000  .\test\1kHz_stereo_p.dsf   | ffplay -f s24le -ar 384k -ch_layout stereo -i -
dsd2dxd -r 96000   .\test\1kHz_stereo_128.dsf | ffplay -f s24le -ar 96k  -ch_layout stereo -i -
dsd2dxd -r 192000  .\test\1kHz_stereo_128.dsf | ffplay -f s24le -ar 192k -ch_layout stereo -i -
dsd2dxd -r 384000  .\test\1kHz_stereo_128.dsf | ffplay -f s24le -ar 384k -ch_layout stereo -i -
dsd2dxd -r 96000   .\test\1kHz_stereo_256.dsf | ffplay -f s24le -ar 96k  -ch_layout stereo -i -
dsd2dxd -r 192000  .\test\1kHz_stereo_256.dsf | ffplay -f s24le -ar 192k -ch_layout stereo -i -
dsd2dxd -r 384000  .\test\1kHz_stereo_256.dsf | ffplay -f s24le -ar 384k -ch_layout stereo -i -

# Run same conversions discarding stdout (PowerShell: redirect to $null)
dsd2dxd -r 96000   -v .\test\1kHz_stereo_p.dsf    > $null
dsd2dxd -r 192000  -v .\test\1kHz_stereo_p.dsf    > $null
dsd2dxd -r 384000  -v .\test\1kHz_stereo_p.dsf    > $null
dsd2dxd -r 96000   -v .\test\1kHz_stereo_128.dsf  > $null
dsd2dxd -r 192000  -v .\test\1kHz_stereo_128.dsf  > $null
dsd2dxd -r 384000  -v .\test\1kHz_stereo_128.dsf  > $null
dsd2dxd -r 96000   -v .\test\1kHz_stereo_256.dsf  > $null
dsd2dxd -r 192000  -v .\test\1kHz_stereo_256.dsf  > $null
dsd2dxd -r 384000  -v .\test\1kHz_stereo_256.dsf  > $null
