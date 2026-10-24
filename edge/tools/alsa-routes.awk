# SPDX-License-Identifier: MIT
# Known OH2P 1.62.2 multiline ALSA layout only. Preserve all unrelated lines.
# mode=check validates; mode=configure changes the two explicit routing fields.
function fail(message) { print "ALSA: " message > "/dev/stderr"; bad=1 }
{
    text=$0
    clean=text
    sub(/#.*/, "", clean)
    if (clean ~ /^[ \t]*pcm[.]mico_record[ \t]*[{][ \t]*$/) { if (block!="") fail("nested pcm declaration"); block="mic"; mic_blocks++; depth=0 }
    if (clean ~ /^[ \t]*pcm[.]noop[ \t]*[{][ \t]*$/) { if (block!="") fail("nested pcm declaration"); block="noop"; noop_blocks++; depth=0 }
    if (clean ~ /^[ \t]*pcm[.]Capture[ \t]*[{][ \t]*$/) { if (block!="") fail("nested pcm declaration"); block="capture"; cap_blocks++; depth=0 }
    if (block=="mic") {
        if (clean ~ /^[ \t]*type[ \t]+plug[ \t]*$/) mic_plug++
        if (clean ~ /^[ \t]*slave[.]pcm[ \t]*[{][ \t]*$/) mic_slave++
        if (clean ~ /^[ \t]*type[ \t]+hw[ \t]*$/) mic_hw++
        if (clean ~ /^[ \t]*card[ \t]+0[ \t]*$/) mic_card++
        if (clean ~ /^[ \t]*device[ \t]+[0-9]+[ \t]*$/) {
            mic_device++
            if (mode=="configure") sub(/device[ \t]+[0-9]+/, "device 0", text)
            else if (clean !~ /device[ \t]+0[ \t]*$/) fail("mico_record must use hw:0,0; pass --configure-alsa to change known layout")
        }
    }
    if (block=="noop") {
        if (clean ~ /^[ \t]*type[ \t]+plug[ \t]*$/) noop_plug++
        if (clean ~ /^[ \t]*slave[.]pcm[ \t]+"?Capture"?[ \t]*$/) noop_capture++
    }
    if (block=="capture") {
        if (clean ~ /^[ \t]*type[ \t]+plug[ \t]*$/) cap_plug++
        if (clean ~ /^[ \t]*slave[.]pcm[ \t]*[{][ \t]*$/) cap_slavepcm++
        if (clean ~ /^[ \t]*type[ \t]+dsnoop[ \t]*$/) cap_dsnoop++
        if (clean ~ /^[ \t]*slave[ \t]*[{][ \t]*$/) cap_slave++
        if (clean ~ /^[ \t]*pcm[ \t]+"hw:0,[0-9]+"[ \t]*$/) {
            cap_pcm++
            if (mode=="configure") sub(/"hw:0,[0-9]+"/, "\"hw:0,3\"", text)
            else if (clean !~ /"hw:0,3"[ \t]*$/) fail("Capture must use hw:0,3; pass --configure-alsa to change known layout")
        }
        if (clean ~ /^[ \t]*rate[ \t]+48000[ \t]*$/) cap_rate++
        if (clean ~ /^[ \t]*format[ \t]+S32_LE[ \t]*$/) cap_format++
        if (clean ~ /^[ \t]*channels[ \t]+4[ \t]*$/) cap_channels++
        if (clean ~ /^[ \t]*period_size[ \t]+384[ \t]*$/) cap_period++
        if (clean ~ /^[ \t]*buffer_size[ \t]+6144[ \t]*$/) cap_buffer++
    }
    if (block!="") {
        opens=gsub(/[{]/,"{",clean); closes=gsub(/[}]/,"}",clean)
        depth+=opens-closes
        if (depth==0) block=""
        if (depth<0) fail("unbalanced target block")
    }
    lines[NR]=text
}
END {
    if (block!="" || mic_blocks!=1 || noop_blocks!=1 || cap_blocks!=1 || mic_plug!=1 || mic_slave!=1 || mic_hw!=1 || mic_card!=1 || mic_device!=1 || noop_plug!=1 || noop_capture!=1 || cap_plug!=1 || cap_slavepcm!=1 || cap_dsnoop!=1 || cap_slave!=1 || cap_pcm!=1 || cap_rate!=1 || cap_format!=1 || cap_channels!=1 || cap_period!=1 || cap_buffer!=1) fail("unrecognized layout; inspect it manually, do not overwrite the complete configuration")
    if (bad) exit 1
    for(i=1;i<=NR;i++) print lines[i]
}
