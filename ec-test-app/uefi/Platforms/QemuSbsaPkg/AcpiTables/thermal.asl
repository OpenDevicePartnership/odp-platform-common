// Sample Definition of FAN ACPI

Device(SKIN) {
  Name(_HID, "MSFT000A")
  
  Name(TVAL,0xdead0001)
  Name(DVAL,0xdead0001)

  Method(_TMP, 0x0, Serialized) {
    If(LEqual(\_SB.FFA0.AVAL,One)) {
      Name(BUFF, Buffer(30){})
    
      CreateByteField(BUFF,0,STAT) // Out – Status for req/rsp 
      CreateByteField(BUFF,1,LENG) // In/Out – Bytes in req, updates bytes returned 
      CreateField(BUFF,16,128,UUID) // UUID of service 
      CreateByteField(BUFF,18,CMDD) // Command register
      CreateByteField(BUFF,19,TZID) // Temp Sensor ID
      CreateDWordField(BUFF,26,RTMP) // Output Data

      Store(20, LENG)
      Store(0x1, CMDD) // EC_THM_GET_TMP
      Store(0x2, TZID) // Temp zone ID for SKIIN
      Store(ToUUID("31f56da7-593c-4d72-a4b3-8fc7171ac073"), UUID)
      Store(Store(BUFF, \_SB_.FFA0.FFAC), BUFF)
      If(LEqual(STAT,0x0) ) // Check FF-A successful?
      {
        Return (RTMP)
      }
    }
    Return (Ones)
  }

  // Arg0 Temp sensor ID
  // Arg1 Package with Low and High set points
  Method(THRS,0x2, Serialized) {
    If(LEqual(\_SB.FFA0.AVAL,One)) {
      Name(BUFF, Buffer(32){})
    
      CreateByteField(BUFF,0,STAT) // Out – Status for req/rsp 
      CreateByteField(BUFF,1,LENG) // In/Out – Bytes in req, updates bytes returned 
      CreateField(BUFF,16,128,UUID) // UUID of service 
      CreateByteField(BUFF,18,CMDD) // Command register
      CreateByteField(BUFF,19,TZID) // Temp Sensor ID
      CreateDwordField(BUFF,20,VTIM) // Timeout
      CreateDwordField(BUFF,24,VLO) // Low Threshold
      CreateDwordField(BUFF,28,VHI) // High Threshold
      CreateDWordField(BUFF,18,TSTS) // Output Data

      Store(ToUUID("31f56da7-593c-4d72-a4b3-8fc7171ac073"), UUID)
      Store(32, LENG)
      Store(0x2, CMDD) // EC_THM_SET_THRS
      Store(Arg0, TZID)
      Store(DeRefOf(Index(Arg1,0)),VTIM)
      Store(DeRefOf(Index(Arg1,1)),VLO)
      Store(DeRefOf(Index(Arg1,2)),VHI)
 
      Store(Store(BUFF, \_SB_.FFA0.FFAC), BUFF)
      If(LEqual(STAT,0x0) ) // Check FF-A successful?
      {
        Return (TSTS)
      }
    }
    Return (Ones)

  }

  // Arg0 GUID  1f0849fc-a845-4fcf-865c-4101bf8e8d79
  // Arg1 Revision
  // Arg2 Function Index
  // Arg3 Function dependent
  Method(_DSM, 0x4, Serialized) {
    If(LEqual(ToUuid("1f0849fc-a845-4fcf-865c-4101bf8e8d79"),Arg0)) {
      Switch(Arg2) {
        Case (0) {
          Return(0x3) // Support Function 0 and Function 1
        }
        Case (1) {
          Return( THRS(0x2, Arg3) ) // Call to function to set threshold
        }
      }
    }
    
    // Drop down to failure case
    Return(Ones)
  }

}

Device(THRM) {
  Name(_HID, "MSFT000B")

  // Arg0 Instance ID
  // Arg1 UUID of variable
  // Return (Status,Value)
  Method(GVAR,2,Serialized) {
    If(LEqual(\_SB.FFA0.AVAL,One)) {
      Name(BUFF, Buffer(38){})
    
      CreateByteField(BUFF,0,STAT) // Out – Status for req/rsp 
      CreateByteField(BUFF,1,LENG) // In/Out – Bytes in req, updates bytes returned 
      CreateField(BUFF,16,128,UUID) // UUID of service 
      CreateByteField(BUFF,18,CMDD) // Command register
      CreateByteField(BUFF,19,INST) // Instance ID
      CreateWordField(BUFF,20,VLEN) // 16-bit variable length
      CreateField(BUFF,176,128,VUID) // UUID of variable to read

      CreateField(BUFF,208,64,RVAL) // Output Data

      Store(ToUUID("31f56da7-593c-4d72-a4b3-8fc7171ac073"), UUID)
      Store(38, LENG)
      Store(0x5, CMDD) // EC_THM_GET_VAR
      Store(Arg0,INST) // Save instance ID
      Store(4,VLEN) // Variable is always DWORD here
      Store(Arg1, VUID)
      Store(Store(BUFF, \_SB_.FFA0.FFAC), BUFF)
      If(LEqual(STAT,0x0) ) // Check FF-A successful?
      {
        Return (RVAL)
      }
    }
    Return (Ones)
  }

  // Arg0 Instance ID
  // Arg1 UUID of variable
  // Return (Status,Value)
  Method(SVAR,3,Serialized) {
    If(LEqual(\_SB.FFA0.AVAL,One)) {
      Name(BUFF, Buffer(42){})
    
      CreateByteField(BUFF,0,STAT) // Out – Status for req/rsp 
      CreateByteField(BUFF,1,LENG) // In/Out – Bytes in req, updates bytes returned 
      CreateField(BUFF,16,128,UUID) // UUID of service 
      CreateByteField(BUFF,18,CMDD) // Command register
      CreateByteField(BUFF,19,INST) // Instance ID
      CreateWordField(BUFF,20,VLEN) // 16-bit variable length
      CreateField(BUFF,176,128,VUID) // UUID of variable to read
      CreateDwordField(BUFF,38,DVAL) // Data value

      CreateField(BUFF,208,32,RVAL) // Ouput Data

      Store(ToUUID("31f56da7-593c-4d72-a4b3-8fc7171ac073"), UUID)
      Store(42, LENG)
      Store(0x6, CMDD) // EC_THM_SET_VAR
      Store(Arg0,INST) // Save instance ID
      Store(4,VLEN) // Variable is always DWORD here
      Store(Arg1, VUID)
      Store(Arg2,DVAL)
      Store(Store(BUFF, \_SB_.FFA0.FFAC), BUFF)
      If(LEqual(STAT,0x0) ) // Check FF-A successful?
      {
        Return (RVAL)
      }
    }
    Return (Ones)
  }


  // Arg0 GUID
  //      07ff6382-e29a-47c9-ac87-e79dad71dd82 - Input
  //      d9b9b7f3-2a3e-4064-8841-cb13d317669e - Output
  // Arg1 Revision
  // Arg2 Function Index
  // Arg3 Function dependent
  Method(_DSM, 0x4, Serialized) {
    // Input Variable
    If(LEqual(ToUuid("07ff6382-e29a-47c9-ac87-e79dad71dd82"),Arg0)) {
        Switch(Arg2) {
          Case(0) {
            // We support function 0-3
            Return(0xf)
          }
          Case(1) {
            Return(GVAR(1,ToUuid("ba17b567-c368-48d5-bc6f-a312a41583c1"))) // OnTemp
          }
          Case(2) {
            Return(GVAR(1,ToUuid("3a62688c-d95b-4d2d-bacc-90d7a5816bcd"))) // RampTemp
          }
          Case(3) {
            Return(GVAR(1,ToUuid("dcb758b1-f0fd-4ec7-b2c0-ef1e2a547b76"))) // MaxTemp
          }

        }
        Return(Ones)
    }
    // Output Variable
    If(LEqual(ToUuid("d9b9b7f3-2a3e-4064-8841-cb13d317669e"),Arg0)) {
        Switch(Arg2) {
          Case(0) {
            // We support function 0-3
            Return(0xf)
          }
          Case(1) {
            Return(SVAR(1,ToUuid("ba17b567-c368-48d5-bc6f-a312a41583c1"),Arg3)) // OnTemp
          }
          Case(2) {
            Return(SVAR(1,ToUuid("3a62688c-d95b-4d2d-bacc-90d7a5816bcd"),Arg3)) // RampTemp
          }
          Case(3) {
            Return(SVAR(1,ToUuid("dcb758b1-f0fd-4ec7-b2c0-ef1e2a547b76"),Arg3)) // MaxTemp
          }
        }
        Return(Ones)
    }

    Return (Ones)
  }


}