#!/bin/bash

# This script waits for the USB/IP device to be available, then attaches it.
# It is designed to be used as a pre-launch task in a debugger, so that
# the device is automatically mounted each time the debugger starts.

set -e
set -u
total_timeout=30
usbip_timeout=1
attach_delay=5
device_name="Clay Logic Nitrokey 3"

endtime=$(($(date +%s) + $total_timeout))


echo "Waiting for USB/IP device to be available..."

# Check if we've tried long enough and should time out
while [ $(date +%s) -le $endtime ]; do

    sleep 0.1

    set +e
    # Get the list of usbip devices, with a timeout.
    output=$(timeout -k $usbip_timeout $usbip_timeout sudo usbip list -r "localhost" 2>&1)
    retval=$?
    set -e

    if [ $retval -eq 124 ] || [ $retval -eq 137 ]; then
        echo "usbip list timed out"
        # The command timed out, which means it's probably already been attached.
        # Check to confirm.
        if lsusb | grep -q "$device_name"; then
            echo "Device attached!"
            exit 0
        else
            >&2 echo "Failed to attach device"
        fi

    elif [ $retval -eq 0 ]; then
        echo "Attaching..."
        # The device is available! Now attach it.
        
        set +e
        sudo usbip list -r "localhost"
        sudo usbip attach -r "localhost" -b "1-1"
        sudo usbip attach -r "localhost" -b "1-1"
        set -e

        sleep $attach_delay

        # Check if it's been attached
        if lsusb | tee /dev/tty | grep -q "$device_name"; then
            echo "Device attached!"
            exit 0
        fi

        # It didn't attach. For some reason, we sometimes have
        # to run this command multiple times for it to work,
        # so start the loop again.
        continue

    elif [ $retval -eq 1 ]; then
        # Couldn't find the port, so keep waiting
        continue

    else
        # Some unexpected error, exit out
        >&2 echo "$output"
        exit $retval
    fi
done

>&2 echo "Failed to find device before timeout"
exit 1
