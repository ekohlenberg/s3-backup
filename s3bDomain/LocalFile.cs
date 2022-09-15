using System;
using System.Collections.Generic;
using System.Text;
using System.Reflection;

namespace s3b
{
    public class LocalFile : Model
    {
        public LocalFile()
        {
            tableName = "local_file";
        }

        public long id
        {
            get
            {
                return Convert.ToInt32(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public long folder_id
        {
            get
            {
                return Convert.ToInt32(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public string full_path
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public DateTime current_update
        {
            get
            {
                return Convert.ToDateTime(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }



        public DateTime previous_update
        {
            get
            {
                return Convert.ToDateTime(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public int exclude
        {
            get
            {
                return Convert.ToInt32(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public void getStdParams(Model cmdParams)
        {
            cmdParams["localfile"] = full_path;
        }
    }
}
